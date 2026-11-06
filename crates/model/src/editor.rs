//! Editing commands: a document, its caret, a selection and the history.
//! Every command turns into [`Op`]s applied through [`History`], so
//! everything is undoable. Geometry (clicks, line moves) is the widget's job;
//! it hands the resulting caret to [`Editor::set_caret`].
//!
//! Math input follows Typst where it can: letters form identifiers until a
//! space or another key ends the word (`log`, `chi` → χ, `\alpha` → α),
//! `/` takes the previous operand or a parenthesized group as numerator,
//! `^`/`_` attach scripts, and `(` … `)` in an empty slot only groups.
//! Raw Typst blocks are not editable here.

use std::collections::HashSet;

use crate::cursor::{CursorPath, Step, Target};
use crate::history::History;
use crate::ops::{Items, Op, Seq};
use crate::symbols;
use crate::tree::{Block, BlockNode, Document, Inline, InlineNode, MathKind, MathNode, NodeId};

/// What the last command was, to group consecutive keystrokes into one undo
/// step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    Other,
    /// Typing; `space` if the last typed char was whitespace.
    Typing {
        space: bool,
    },
    /// Deleting single chars with Backspace or Delete.
    Deleting,
}

/// A selected subtree, shown highlighted; Backspace/Delete remove it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selected {
    /// Nodes `start..end` of the math row at `path`.
    Nodes {
        block: NodeId,
        path: Vec<Step>,
        start: usize,
        end: usize,
    },
    /// Inline formula number `inline` of a paragraph.
    Formula { block: NodeId, inline: usize },
}

#[derive(Debug)]
pub struct Editor {
    doc: Document,
    caret: CursorPath,
    history: History,
    last: Last,
    selected: Option<Selected>,
    /// Caret position right after an identifier that is still being typed.
    word: Option<CursorPath>,
    /// Rows opened with a `(` that was swallowed; `)` leaves them.
    groups: HashSet<NodeId>,
    /// Typst's symbol names with modifiers (`times.o` → ⊗), provided by the
    /// GUI from the Typst bridge. Without it, a small built-in table is used.
    resolver: Option<SymbolResolver>,
}

/// Looks up a symbol name with `.modifiers`; `None` if it is not a symbol.
pub type SymbolResolver = fn(&str) -> Option<String>;

/// Position of a text caret inside a paragraph or heading.
struct TextPos<'a> {
    block_index: usize,
    block: &'a BlockNode,
    inlines: &'a [InlineNode],
    inline: usize,
    offset: usize,
    len: usize,
}

impl TextPos<'_> {
    fn chars(&self) -> Vec<char> {
        text_of(&self.inlines[self.inline]).chars().collect()
    }
}

/// Position of a caret in a math row.
#[derive(Clone, Debug)]
struct RowPos {
    block_index: usize,
    block: NodeId,
    /// Length of the path prefix before the formula's row path (1 for an
    /// inline formula, 0 for display math).
    base: usize,
    path: Vec<Step>,
    row: NodeId,
    offset: usize,
    len: usize,
}

impl RowPos {
    fn nested(&self) -> bool {
        self.path.len() > self.base
    }

    /// Path of the parent row, index of the structure in it, and the step
    /// from the structure into this row.
    fn parent(&self) -> Option<(Vec<Step>, usize, Step)> {
        if !self.nested() {
            return None;
        }
        let n = self.path.len();
        let Step::Index(j) = self.path[n - 2] else {
            return None;
        };
        Some((self.path[..n - 2].to_vec(), j, self.path[n - 1]))
    }

    fn at(&self, offset: usize) -> CursorPath {
        cursor(self.block, self.path.clone(), offset)
    }

    fn child(&self, index: usize, step: Step) -> CursorPath {
        cursor(self.block, child_path(&self.path, index, step), 0)
    }
}

fn cursor(block: NodeId, path: Vec<Step>, offset: usize) -> CursorPath {
    CursorPath {
        block,
        path,
        offset,
    }
}

fn child_path(path: &[Step], index: usize, step: Step) -> Vec<Step> {
    let mut p = path.to_vec();
    p.extend([Step::Index(index), step]);
    p
}

fn row_insert(row: NodeId, index: usize, node: MathNode) -> Op {
    Op::Insert {
        seq: Seq::Row(row),
        index,
        items: Items::Math(vec![node]),
    }
}

fn row_delete(row: NodeId, index: usize, count: usize) -> Op {
    Op::Delete {
        seq: Seq::Row(row),
        index,
        count,
    }
}

fn row_replace(row: NodeId, index: usize, node: MathNode) -> Op {
    Op::Replace {
        seq: Seq::Row(row),
        index,
        count: 1,
        items: Items::Math(vec![node]),
    }
}

fn wrap(row: NodeId, index: usize, count: usize, wrapper: MathNode, slot: Step) -> Op {
    Op::Wrap {
        row,
        index,
        count,
        wrapper,
        slot,
    }
}

/// Nodes that `/`, `^` and `_` take as their operand.
fn is_operand(node: &MathNode) -> bool {
    match &node.kind {
        MathKind::Sym(s) => symbols::is_letterlike(s),
        _ => true,
    }
}

fn is_sym(node: &MathNode, s: &str) -> bool {
    matches!(&node.kind, MathKind::Sym(x) if x == s)
}

/// If the node before `offset` is `)`, the index of its matching `(`.
fn group_before(nodes: &[MathNode], offset: usize) -> Option<usize> {
    let close = offset.checked_sub(1)?;
    if !is_sym(&nodes[close], ")") {
        return None;
    }
    let mut depth = 0;
    for j in (0..close).rev() {
        if is_sym(&nodes[j], ")") {
            depth += 1;
        } else if is_sym(&nodes[j], "(") {
            if depth == 0 {
                return Some(j);
            }
            depth -= 1;
        }
    }
    None
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(Document::new())
    }
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        let caret = doc.block_start(0);
        Self {
            doc,
            caret,
            history: History::new(),
            last: Last::Other,
            selected: None,
            word: None,
            groups: HashSet::new(),
            resolver: None,
        }
    }

    pub fn set_symbol_resolver(&mut self, resolver: SymbolResolver) {
        self.resolver = Some(resolver);
    }

    /// The symbol for a word (`\\`-commands first try the TeX table).
    fn lookup(&self, name: &str, command: bool) -> Option<String> {
        if command && let Some(s) = symbols::word(name, true) {
            return Some(s.into());
        }
        match self.resolver {
            Some(resolve) => resolve(name),
            None => symbols::word(name, command).map(Into::into),
        }
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn caret(&self) -> &CursorPath {
        &self.caret
    }

    pub fn selected(&self) -> Option<&Selected> {
        self.selected.as_ref()
    }

    /// Move the caret (e.g. after a click). Invalid carets are repaired.
    pub fn set_caret(&mut self, caret: CursorPath) {
        self.finish_word();
        self.selected = None;
        self.caret = self.doc.fix_cursor(&caret);
        self.last = Last::Other;
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    fn reset_state(&mut self) {
        self.last = Last::Other;
        self.selected = None;
        self.word = None;
        self.groups.clear();
    }

    /// Returns whether anything changed.
    pub fn undo(&mut self) -> bool {
        self.finish_word();
        self.reset_state();
        match self.history.undo(&mut self.doc) {
            Some(caret) => {
                self.caret = self.doc.fix_cursor(&caret);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.reset_state();
        match self.history.redo(&mut self.doc) {
            Some(caret) => {
                self.caret = self.doc.fix_cursor(&caret);
                true
            }
            None => false,
        }
    }

    fn text_pos(&self) -> Option<TextPos<'_>> {
        let c = &self.caret;
        let block_index = self.doc.block_index(c.block)?;
        let block = &self.doc.blocks[block_index];
        let inlines = block.kind.inlines()?;
        let [Step::Index(inline)] = c.path[..] else {
            return None;
        };
        let Inline::Text(text) = &inlines.get(inline)?.kind else {
            return None;
        };
        Some(TextPos {
            block_index,
            block,
            inlines,
            inline,
            offset: c.offset,
            len: text.chars().count(),
        })
    }

    fn row_pos(&self) -> Option<RowPos> {
        let c = &self.caret;
        let block_index = self.doc.block_index(c.block)?;
        let block = &self.doc.blocks[block_index];
        let Ok(Target::Row { row }) = self.doc.walk(block, &c.path) else {
            return None;
        };
        Some(RowPos {
            block_index,
            block: block.id,
            base: usize::from(block.kind.inlines().is_some()),
            path: c.path.clone(),
            row: row.id,
            offset: c.offset,
            len: row.nodes.len(),
        })
    }

    fn row_nodes(&self, row: NodeId) -> &[MathNode] {
        self.doc.find_row(row).map_or(&[][..], |r| &r.nodes)
    }

    fn row_id_at(&self, block: NodeId, path: &[Step]) -> Option<NodeId> {
        match self.doc.walk(self.doc.block(block)?, path) {
            Ok(Target::Row { row }) => Some(row.id),
            _ => None,
        }
    }

    /// Run ops as one undo step (or merged into the previous one) and move
    /// the caret.
    fn commit(&mut self, ops: Vec<Op>, after: CursorPath, last: Last) -> bool {
        let merge = match (self.last, last) {
            (Last::Typing { space: was }, Last::Typing { space: now }) => !was || now,
            (Last::Deleting, Last::Deleting) => true,
            _ => false,
        };
        let before = self.caret.clone();
        let result = if merge {
            self.history.apply_merged(&mut self.doc, ops, after.clone())
        } else {
            self.history
                .apply(&mut self.doc, ops, before, after.clone())
        };
        match result {
            Ok(()) => {
                debug_assert!(self.doc.check_invariants().is_ok());
                self.caret = self.doc.fix_cursor(&after);
                self.last = last;
                self.selected = None;
                true
            }
            Err(e) => {
                if cfg!(debug_assertions) {
                    panic!("edit failed: {e}");
                }
                false
            }
        }
    }

    fn jump(&mut self, caret: CursorPath) -> bool {
        self.last = Last::Other;
        self.selected = None;
        let changed = caret != self.caret;
        self.caret = caret;
        changed
    }

    // ----- Typing -----

    /// Insert typed text at the caret, char by char: prose in text, structural
    /// input in math.
    pub fn insert_text(&mut self, s: &str) -> bool {
        let mut changed = false;
        for c in s.chars() {
            changed |= self.input_char(c);
        }
        changed
    }

    fn input_char(&mut self, c: char) -> bool {
        if self.row_pos().is_some() {
            return self.math_char(c);
        }
        self.selected = None;
        if c == '$' {
            return self.insert_formula();
        }
        self.text_insert(&c.to_string())
    }

    /// Typst collapses runs of spaces and drops spaces at the start of a
    /// paragraph, so such spaces are not inserted at all: the model only
    /// holds text that is visible.
    fn text_insert(&mut self, s: &str) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let chars = pos.chars();
        // `= ` at the start of a paragraph makes it a heading (as in Typst).
        let level = pos.offset;
        if s == " "
            && pos.inline == 0
            && (1..=6).contains(&level)
            && chars[..level].iter().all(|&c| c == '=')
            && matches!(pos.block.kind, Block::Paragraph(_))
        {
            let b = pos.block_index;
            let after = cursor(pos.block.id, vec![Step::Index(0)], 0);
            return self.retype(b, Some(level as u8), level, after);
        }
        let mut space_before = pos.offset == 0 && pos.inline == 0
            || pos.offset > 0 && chars[pos.offset - 1].is_whitespace();
        let mut out = String::new();
        for c in s.chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c.is_control() || c == ' ' && space_before {
                continue;
            }
            space_before = c == ' ';
            out.push(c);
        }
        if chars.get(pos.offset).is_some_and(|c| c.is_whitespace()) && out.ends_with(' ') {
            out.pop();
        }
        let s = out;
        if s.is_empty() {
            return false;
        }
        let id = pos.inlines[pos.inline].id;
        let n = s.chars().count();
        let space = s.ends_with(char::is_whitespace);
        let after = CursorPath {
            offset: pos.offset + n,
            ..self.caret.clone()
        };
        let op = Op::Insert {
            seq: Seq::Text(id),
            index: pos.offset,
            items: Items::Text(s),
        };
        self.commit(vec![op], after, Last::Typing { space })
    }

    /// `$` in text: an empty inline formula at the caret, caret inside.
    fn insert_formula(&mut self) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let (i, c, len) = (pos.inline, pos.offset, pos.len);
        let block = pos.block.id;
        let text_id = pos.inlines[i].id;
        let tail: String = pos.chars()[c..].iter().collect();
        let mut ops = vec![];
        if c < len {
            ops.push(Op::Delete {
                seq: Seq::Text(text_id),
                index: c,
                count: len - c,
            });
        }
        let math = self.doc.math(vec![]);
        let rest = self.doc.text(&tail);
        ops.push(Op::Insert {
            seq: Seq::Inlines(block),
            index: i + 1,
            items: Items::Inlines(vec![math, rest]),
        });
        let after = cursor(block, vec![Step::Index(i + 1)], 0);
        self.commit(ops, after, Last::Other)
    }

    fn math_char(&mut self, c: char) -> bool {
        let mut changed = false;
        if !matches!(c, '/' | '^' | '_') && matches!(self.selected, Some(Selected::Nodes { .. })) {
            changed |= self.delete_selection();
        }
        let Some(p) = self.row_pos() else {
            return changed;
        };
        let typing = Last::Typing { space: false };
        let prev = |e: &Self| {
            p.offset
                .checked_sub(1)
                .and_then(|k| e.row_nodes(p.row).get(k))
                .cloned()
        };

        if c.is_ascii_alphabetic() {
            if self.word.as_ref() == Some(&self.caret)
                && let Some(MathNode {
                    id,
                    kind: MathKind::Ident(name),
                }) = prev(self)
            {
                let node = MathNode {
                    id,
                    kind: MathKind::Ident(format!("{name}{c}")),
                };
                let ok = self.commit(
                    vec![row_replace(p.row, p.offset - 1, node)],
                    p.at(p.offset),
                    typing,
                );
                self.word = Some(self.caret.clone());
                return ok;
            }
            self.finish_word();
            let Some(p) = self.row_pos() else {
                return true;
            };
            let node = self.doc.ident(&c.to_string());
            self.commit(
                vec![row_insert(p.row, p.offset, node)],
                p.at(p.offset + 1),
                typing,
            );
            self.word = Some(self.caret.clone());
            return true;
        }

        // `.` continues a symbol name with a modifier: `times.o`, `arrow.r`.
        if c == '.'
            && self.word.as_ref() == Some(&self.caret)
            && let Some(MathNode {
                id,
                kind: MathKind::Ident(name),
            }) = prev(self)
            && self
                .lookup(name.trim_start_matches('\\'), name.starts_with('\\'))
                .is_some()
        {
            let node = MathNode {
                id,
                kind: MathKind::Ident(format!("{name}.")),
            };
            let op = row_replace(p.row, p.offset - 1, node);
            let ok = self.commit(vec![op], p.at(p.offset), typing);
            self.word = Some(self.caret.clone());
            return ok;
        }

        // Finishing a word can remove a lone `\`, so look again.
        changed |= self.finish_word();
        let Some(p) = self.row_pos() else {
            return true;
        };
        let prev = p
            .offset
            .checked_sub(1)
            .and_then(|k| self.row_nodes(p.row).get(k))
            .cloned();
        match c {
            '$' => self.dollar() || changed,
            '/' => self.fraction(&p) || changed,
            '^' => self.script(&p, Step::Sup) || changed,
            '_' => self.script(&p, Step::Sub) || changed,
            '\\' => {
                let node = self.doc.ident("\\");
                self.commit(
                    vec![row_insert(p.row, p.offset, node)],
                    p.at(p.offset + 1),
                    typing,
                );
                self.word = Some(self.caret.clone());
                true
            }
            '(' if p.nested() && p.len == 0 => {
                self.groups.insert(p.row);
                true
            }
            ')' if self.groups.contains(&p.row) => self.escape() || changed,
            c if c.is_whitespace() => changed,
            c if c.is_ascii_digit() || c == '.' => {
                if let Some(MathNode {
                    id,
                    kind: MathKind::Num(n),
                }) = &prev
                    && (c != '.' || !n.contains('.'))
                {
                    let node = MathNode {
                        id: *id,
                        kind: MathKind::Num(format!("{n}{c}")),
                    };
                    let op = row_replace(p.row, p.offset - 1, node);
                    return self.commit(vec![op], p.at(p.offset), typing);
                }
                let node = if c == '.' {
                    self.doc.sym(".")
                } else {
                    self.doc.num(&c.to_string())
                };
                self.commit(
                    vec![row_insert(p.row, p.offset, node)],
                    p.at(p.offset + 1),
                    typing,
                )
            }
            c if c.is_control() => changed,
            c if c.is_alphabetic() => {
                let node = self.doc.ident(&c.to_string());
                self.commit(
                    vec![row_insert(p.row, p.offset, node)],
                    p.at(p.offset + 1),
                    typing,
                )
            }
            c => {
                if let Some(MathNode {
                    id,
                    kind: MathKind::Sym(s),
                }) = &prev
                    && let Some(sym) = symbols::pair(s, c)
                {
                    let node = MathNode {
                        id: *id,
                        kind: MathKind::Sym(sym.into()),
                    };
                    let op = row_replace(p.row, p.offset - 1, node);
                    return self.commit(vec![op], p.at(p.offset), typing);
                }
                let node = self.doc.sym(&symbols::typed_symbol(c));
                self.commit(
                    vec![row_insert(p.row, p.offset, node)],
                    p.at(p.offset + 1),
                    typing,
                )
            }
        }
    }

    /// End the identifier being typed: a known name becomes its symbol.
    fn finish_word(&mut self) -> bool {
        let Some(w) = self.word.take() else {
            return false;
        };
        if w != self.caret {
            return false;
        }
        let Some(p) = self.row_pos() else {
            return false;
        };
        let Some(MathNode {
            id,
            kind: MathKind::Ident(name),
        }) = p
            .offset
            .checked_sub(1)
            .and_then(|k| self.row_nodes(p.row).get(k))
            .cloned()
        else {
            return false;
        };
        let (bare, command) = match name.strip_prefix('\\') {
            Some(n) => (n, true),
            None => (name.as_str(), false),
        };
        let typing = Last::Typing { space: false };
        let kind = match self.lookup(bare, command) {
            Some(sym) => MathKind::Sym(sym),
            None if bare.contains('.') => {
                // Not a symbol after all (`x.`, `times.q`): the part before
                // the dot as usual, then the dot, then the rest.
                let (head, tail) = bare.split_once('.').expect("contains a dot");
                let head = match self.lookup(head, command) {
                    Some(sym) => MathKind::Sym(sym),
                    None => MathKind::Ident(head.into()),
                };
                let mut nodes = vec![MathNode { id, kind: head }, self.doc.sym(".")];
                if !tail.is_empty() {
                    nodes.push(self.doc.ident(tail));
                }
                let n = nodes.len();
                let op = Op::Replace {
                    seq: Seq::Row(p.row),
                    index: p.offset - 1,
                    count: 1,
                    items: Items::Math(nodes),
                };
                return self.commit(vec![op], p.at(p.offset - 1 + n), typing);
            }
            None if command && !bare.is_empty() => MathKind::Ident(bare.into()),
            None if command => {
                let op = row_delete(p.row, p.offset - 1, 1);
                return self.commit(vec![op], p.at(p.offset - 1), typing);
            }
            None => return false,
        };
        let op = row_replace(p.row, p.offset - 1, MathNode { id, kind });
        self.commit(vec![op], p.at(p.offset), typing)
    }

    /// The selected node range if it lies in the caret's row.
    fn take_selection_in(&mut self, p: &RowPos) -> Option<(usize, usize)> {
        match self.selected.take() {
            Some(Selected::Nodes {
                block,
                path,
                start,
                end,
                ..
            }) if block == p.block && path == p.path => Some((start, end)),
            _ => None,
        }
    }

    /// `/`: the selection, a parenthesized group or the previous operand
    /// becomes the numerator and the caret goes to the denominator. With
    /// nothing to take, an empty fraction with the caret in the numerator.
    fn fraction(&mut self, p: &RowPos) -> bool {
        let nodes = self.row_nodes(p.row).to_vec();
        let wrapper = self.doc.frac(vec![], vec![]);
        let (ops, at) = if let Some((s, e)) = self.take_selection_in(p) {
            (vec![wrap(p.row, s, e - s, wrapper, Step::Num)], s)
        } else if let Some(open) = group_before(&nodes, p.offset) {
            // `(a b)/` → a b over …, without the parentheses (as in Typst).
            let close = p.offset - 1;
            let ops = vec![
                row_delete(p.row, close, 1),
                wrap(p.row, open + 1, close - open - 1, wrapper, Step::Num),
                row_delete(p.row, open, 1),
            ];
            (ops, open)
        } else if p.offset > 0 && is_operand(&nodes[p.offset - 1]) {
            (
                vec![wrap(p.row, p.offset - 1, 1, wrapper, Step::Num)],
                p.offset - 1,
            )
        } else {
            let op = row_insert(p.row, p.offset, wrapper);
            return self.commit(vec![op], p.child(p.offset, Step::Num), Last::Other);
        };
        self.commit(ops, p.child(at, Step::Den), Last::Other)
    }

    /// `^` / `_`: attach a script to the selection, group or previous
    /// operand (or add it to existing scripts) and put the caret inside.
    fn script(&mut self, p: &RowPos, step: Step) -> bool {
        let nodes = self.row_nodes(p.row).to_vec();
        let wrapper = match step {
            Step::Sup => self.doc.scripts(vec![], Some(vec![]), None),
            _ => self.doc.scripts(vec![], None, Some(vec![])),
        };
        let (s, count) = if let Some((s, e)) = self.take_selection_in(p) {
            (s, e - s)
        } else if let Some(k) = p.offset.checked_sub(1)
            && let MathKind::Scripts { .. } = &nodes[k].kind
        {
            if let Some(row) = nodes[k].child(step) {
                let caret = cursor(p.block, child_path(&p.path, k, step), row.nodes.len());
                return self.jump(caret);
            }
            let mut node = nodes[k].clone();
            let row = self.doc.row(vec![]);
            if let MathKind::Scripts { sup, sub, .. } = &mut node.kind {
                *(if step == Step::Sup { sup } else { sub }) = Some(row);
            }
            return self.commit(
                vec![row_replace(p.row, k, node)],
                p.child(k, step),
                Last::Other,
            );
        } else if let Some(open) = group_before(&nodes, p.offset) {
            (open, p.offset - open)
        } else if p.offset > 0 && is_operand(&nodes[p.offset - 1]) {
            (p.offset - 1, 1)
        } else {
            let op = row_insert(p.row, p.offset, wrapper);
            return self.commit(vec![op], p.child(p.offset, step), Last::Other);
        };
        let op = wrap(p.row, s, count, wrapper, Step::Base);
        self.commit(vec![op], p.child(s, step), Last::Other)
    }

    /// `$` in math: leave the formula; in an empty inline formula, turn it
    /// into a display formula instead (`$$`).
    fn dollar(&mut self) -> bool {
        let Some(p) = self.row_pos() else {
            return false;
        };
        if !p.nested() && p.base == 1 && p.len == 0 {
            return self.make_display(&p);
        }
        self.leave_formula(&p)
    }

    fn make_display(&mut self, p: &RowPos) -> bool {
        let b = p.block_index;
        let Step::Index(i) = p.path[0] else {
            return false;
        };
        let block = &self.doc.blocks[b];
        let block_id = block.id;
        let inlines = block
            .kind
            .inlines()
            .expect("inline formula in a text block");
        let n = inlines.len();
        let mut tail: Vec<InlineNode> = inlines[i + 1..].to_vec();
        let before_empty = i == 1 && text_of(&inlines[0]).is_empty();
        let tail_empty = tail.len() == 1 && text_of(&tail[0]).trim().is_empty();
        if let Some(InlineNode {
            kind: Inline::Text(t),
            ..
        }) = tail.first_mut()
        {
            *t = t.trim_start().into();
        }
        let mut ops = vec![Op::Delete {
            seq: Seq::Inlines(block_id),
            index: i,
            count: n - i,
        }];
        let display = self.doc.display(vec![]);
        let after = cursor(display.id, vec![], 0);
        let mut blocks = vec![display];
        if !tail_empty {
            blocks.push(self.doc.paragraph(tail));
        }
        ops.push(Op::Insert {
            seq: Seq::Blocks,
            index: b + 1,
            items: Items::Blocks(blocks),
        });
        if before_empty {
            ops.push(delete_block(b));
        }
        self.commit(ops, after, Last::Other)
    }

    /// Caret after the formula: in the text after an inline formula, or in
    /// the paragraph after a display formula (created if needed).
    fn leave_formula(&mut self, p: &RowPos) -> bool {
        self.groups.clear();
        if p.base == 1 {
            let Step::Index(i) = p.path[0] else {
                return false;
            };
            return self.jump(cursor(p.block, vec![Step::Index(i + 1)], 0));
        }
        let b = p.block_index;
        if self
            .doc
            .blocks
            .get(b + 1)
            .is_some_and(|n| n.kind.inlines().is_some())
        {
            let caret = self.doc.block_start(b + 1);
            return self.jump(caret);
        }
        self.paragraph_after(b)
    }

    fn paragraph_after(&mut self, b: usize) -> bool {
        let block = self.doc.paragraph(vec![]);
        let after = cursor(block.id, vec![Step::Index(0)], 0);
        let op = Op::Insert {
            seq: Seq::Blocks,
            index: b + 1,
            items: Items::Blocks(vec![block]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// Escape: drop the selection, else leave the innermost structure (caret
    /// just after it), else leave the formula.
    pub fn escape(&mut self) -> bool {
        self.finish_word();
        if self.selected.take().is_some() {
            return true;
        }
        let Some(p) = self.row_pos() else {
            return false;
        };
        self.groups.remove(&p.row);
        match p.parent() {
            Some((parent, j, _)) => self.jump(cursor(p.block, parent, j + 1)),
            None => self.leave_formula(&p),
        }
    }

    // ----- Deleting -----

    /// Remove the selection.
    fn delete_selection(&mut self) -> bool {
        let Some(sel) = self.selected.take() else {
            return false;
        };
        match sel {
            Selected::Nodes {
                block,
                path,
                start,
                end,
            } => {
                let Some(row) = self.row_id_at(block, &path) else {
                    return false;
                };
                let op = row_delete(row, start, end - start);
                self.commit(vec![op], cursor(block, path, start), Last::Other)
            }
            Selected::Formula { block, inline } => {
                let Some(inlines) = self.doc.block(block).and_then(|b| b.kind.inlines()) else {
                    return false;
                };
                let (ops, after) = remove_formula(block, inlines, inline);
                self.commit(ops, after, Last::Other)
            }
        }
    }

    /// Delete the char or node before the caret. Structures and formulas are
    /// selected by the first press and deleted by the second; an empty slot
    /// is removed and its structure unwrapped.
    pub fn backspace(&mut self) -> bool {
        if self.selected.is_some() {
            return self.delete_selection();
        }
        if let Some(p) = self.row_pos() {
            return self.math_backspace(p);
        }
        let Some(pos) = self.text_pos() else {
            return self.backspace_block();
        };
        let block_id = pos.block.id;
        if pos.offset > 0 {
            let (op, after) = self.delete_char(&pos, pos.offset - 1);
            return self.commit(vec![op], after, Last::Deleting);
        }
        if pos.inline == 0 && matches!(pos.block.kind, Block::Heading { .. }) {
            // A heading first becomes a paragraph; the next press joins.
            let (b, after) = (pos.block_index, self.caret.clone());
            return self.retype(b, None, 0, after);
        }
        if pos.inline >= 2 {
            let inline = pos.inline - 1;
            self.last = Last::Other;
            self.selected = Some(Selected::Formula {
                block: block_id,
                inline,
            });
            return true;
        }
        let b = pos.block_index;
        if b == 0 {
            return false;
        }
        if self.doc.blocks[b - 1].kind.inlines().is_some() {
            let (ops, after) = join_blocks(&self.doc, b - 1);
            return self.commit(ops, after, Last::Other);
        }
        // The previous block is math or raw Typst.
        if is_empty_text_block(pos.block) {
            let after = self.doc.block_end(b - 1);
            let ops = vec![delete_block(b)];
            return self.commit(ops, after, Last::Other);
        }
        let caret = self.doc.block_end(b - 1);
        self.jump(caret)
    }

    fn math_backspace(&mut self, p: RowPos) -> bool {
        let nodes = self.row_nodes(p.row).to_vec();
        if p.offset > 0 {
            let k = p.offset - 1;
            let node = &nodes[k];
            if node.is_structure() {
                self.finish_word();
                self.last = Last::Other;
                self.selected = Some(Selected::Nodes {
                    block: p.block,
                    path: p.path.clone(),
                    start: k,
                    end: k + 1,
                });
                return true;
            }
            let in_word = self.word.as_ref() == Some(&self.caret);
            if let MathKind::Ident(s) | MathKind::Num(s) = &node.kind
                && s.chars().count() > 1
            {
                let mut short = s.clone();
                short.pop();
                let kind = match node.kind {
                    MathKind::Ident(_) => MathKind::Ident(short),
                    _ => MathKind::Num(short),
                };
                let op = row_replace(p.row, k, MathNode { id: node.id, kind });
                let ok = self.commit(vec![op], p.at(p.offset), Last::Deleting);
                if in_word {
                    self.word = Some(self.caret.clone());
                }
                return ok;
            }
            self.word = None;
            return self.commit(vec![row_delete(p.row, k, 1)], p.at(k), Last::Deleting);
        }
        self.finish_word();
        if p.nested() {
            return if p.len == 0 {
                self.remove_slot(&p)
            } else {
                self.move_left()
            };
        }
        let b = p.block_index;
        if p.base == 1 {
            let Step::Index(i) = p.path[0] else {
                return false;
            };
            if p.len > 0 {
                return self.move_left();
            }
            let inlines = self.doc.blocks[b].kind.inlines().expect("text block");
            let (ops, after) = remove_formula(p.block, inlines, i);
            return self.commit(ops, after, Last::Other);
        }
        // Display math.
        if p.len == 0 {
            let (ops, after) = if self.doc.blocks.len() == 1 {
                let para = self.doc.paragraph(vec![]);
                let after = cursor(para.id, vec![Step::Index(0)], 0);
                let insert = Op::Insert {
                    seq: Seq::Blocks,
                    index: 1,
                    items: Items::Blocks(vec![para]),
                };
                (vec![insert, delete_block(0)], after)
            } else if b > 0 {
                (vec![delete_block(b)], self.doc.block_end(b - 1))
            } else {
                (vec![delete_block(b)], self.doc.block_start(b + 1))
            };
            return self.commit(ops, after, Last::Other);
        }
        if b > 0 && is_empty_text_block(&self.doc.blocks[b - 1]) {
            let after = self.caret.clone();
            return self.commit(vec![delete_block(b - 1)], after, Last::Other);
        }
        self.move_left()
    }

    /// Backspace in an empty slot: remove it, unwrapping the structure when
    /// only one meaningful part remains.
    fn remove_slot(&mut self, p: &RowPos) -> bool {
        let Some((parent, j, step)) = p.parent() else {
            return false;
        };
        let Some(prow) = self.row_id_at(p.block, &parent) else {
            return false;
        };
        let node = self.row_nodes(prow)[j].clone();
        let at = |offset| cursor(p.block, parent.clone(), offset);
        let remove = row_delete(prow, j, 1);
        let unwrap = |slot| Op::Unwrap {
            row: prow,
            index: j,
            slot,
        };
        let (ops, after) = match &node.kind {
            MathKind::Frac { num, den } => {
                let (other_step, other) = match step {
                    Step::Num => (Step::Den, den),
                    _ => (Step::Num, num),
                };
                if other.nodes.is_empty() {
                    (vec![remove], at(j))
                } else if step == Step::Den {
                    (vec![unwrap(other_step)], at(j + other.nodes.len()))
                } else {
                    (vec![unwrap(other_step)], at(j))
                }
            }
            MathKind::Scripts { base, sup, sub } => match step {
                Step::Sup | Step::Sub => {
                    let other = if step == Step::Sup { sub } else { sup };
                    if other.is_some() {
                        let mut n = node.clone();
                        if let MathKind::Scripts { sup, sub, .. } = &mut n.kind {
                            *(if step == Step::Sup { sup } else { sub }) = None;
                        }
                        let base_end = cursor(
                            p.block,
                            child_path(&parent, j, Step::Base),
                            base.nodes.len(),
                        );
                        (vec![row_replace(prow, j, n)], base_end)
                    } else if base.nodes.is_empty() {
                        (vec![remove], at(j))
                    } else {
                        (vec![unwrap(Step::Base)], at(j + base.nodes.len()))
                    }
                }
                _ => {
                    let empty = |r: &Option<crate::tree::MathRow>| {
                        r.as_ref().is_none_or(|r| r.nodes.is_empty())
                    };
                    if empty(sup) && empty(sub) {
                        (vec![remove], at(j))
                    } else {
                        return self.move_left();
                    }
                }
            },
            _ => return false,
        };
        self.commit(ops, after, Last::Other)
    }

    /// Delete char `k` of the caret's text; also a space that would end up
    /// doubled or leading the paragraph. Returns the op and the new caret.
    fn delete_char(&self, pos: &TextPos, k: usize) -> (Op, CursorPath) {
        let chars = pos.chars();
        let ws = |i: usize| chars.get(i).is_some_and(|c| c.is_whitespace());
        let leading = k == 0 && pos.inline == 0 && ws(1);
        let doubled = k > 0 && ws(k - 1) && ws(k + 1);
        let (index, count) = if doubled {
            (k - 1, 2)
        } else if leading {
            (0, 2)
        } else {
            (k, 1)
        };
        let op = Op::Delete {
            seq: Seq::Text(pos.inlines[pos.inline].id),
            index,
            count,
        };
        let after = CursorPath {
            offset: index,
            ..self.caret.clone()
        };
        (op, after)
    }

    /// Backspace at the start of a raw block: an empty paragraph before it
    /// is removed, otherwise the caret moves back.
    fn backspace_block(&mut self) -> bool {
        let Some(b) = self.doc.block_index(self.caret.block) else {
            return false;
        };
        if self.caret.offset > 0 || !self.caret.path.is_empty() || b == 0 {
            return false;
        }
        if is_empty_text_block(&self.doc.blocks[b - 1]) {
            let after = self.caret.clone();
            return self.commit(vec![delete_block(b - 1)], after, Last::Other);
        }
        let caret = self.doc.block_end(b - 1);
        self.jump(caret)
    }

    /// Delete the char or node after the caret (structures and formulas:
    /// select first); at the end of a text, join with the next block.
    pub fn delete(&mut self) -> bool {
        if self.selected.is_some() {
            return self.delete_selection();
        }
        if let Some(p) = self.row_pos() {
            self.finish_word();
            let Some(node) = self.row_nodes(p.row).get(p.offset) else {
                return false;
            };
            if node.is_structure() {
                self.last = Last::Other;
                self.selected = Some(Selected::Nodes {
                    block: p.block,
                    path: p.path.clone(),
                    start: p.offset,
                    end: p.offset + 1,
                });
                return true;
            }
            return self.commit(
                vec![row_delete(p.row, p.offset, 1)],
                p.at(p.offset),
                Last::Deleting,
            );
        }
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let block_id = pos.block.id;
        if pos.offset < pos.len {
            let (op, after) = self.delete_char(&pos, pos.offset);
            return self.commit(vec![op], after, Last::Deleting);
        }
        if pos.inline + 2 < pos.inlines.len() {
            let inline = pos.inline + 1;
            self.last = Last::Other;
            self.selected = Some(Selected::Formula {
                block: block_id,
                inline,
            });
            return true;
        }
        let b = pos.block_index;
        let Some(next) = self.doc.blocks.get(b + 1) else {
            return false;
        };
        if next.kind.inlines().is_some() {
            let (ops, after) = join_blocks(&self.doc, b);
            return self.commit(ops, after, Last::Other);
        }
        if is_empty_text_block(pos.block) {
            let after = self.doc.block_start(b + 1);
            return self.commit(vec![delete_block(b)], after, Last::Other);
        }
        false
    }

    // ----- Blocks -----

    /// Split the paragraph at the caret. Inside an inline formula, split
    /// after it; in a display formula or raw block, start a new paragraph.
    pub fn enter(&mut self) -> bool {
        self.finish_word();
        self.selected = None;
        if let Some(p) = self.row_pos() {
            if p.base == 1 {
                let Step::Index(i) = p.path[0] else {
                    return false;
                };
                self.caret = cursor(p.block, vec![Step::Index(i + 1)], 0);
            } else {
                return self.paragraph_after(p.block_index);
            }
        }
        let Some(pos) = self.text_pos() else {
            let Some(b) = self.doc.block_index(self.caret.block) else {
                return false;
            };
            return self.paragraph_after(b);
        };
        let (b, i, c, len) = (pos.block_index, pos.inline, pos.offset, pos.len);
        let block_id = pos.block.id;
        let inlines = pos.inlines;
        let n = inlines.len();
        let tail: String = pos.chars()[c..].iter().collect();
        let moved: Vec<InlineNode> = inlines[i + 1..].to_vec();
        let heading = match &pos.block.kind {
            // Enter at the very end of a heading starts a paragraph.
            Block::Heading { level, .. } if !(c == len && i + 1 == n) => Some(*level),
            _ => None,
        };
        let text_id = inlines[i].id;

        let mut ops = vec![];
        if c < len {
            ops.push(Op::Delete {
                seq: Seq::Text(text_id),
                index: c,
                count: len - c,
            });
        }
        if i + 1 < n {
            ops.push(Op::Delete {
                seq: Seq::Inlines(block_id),
                index: i + 1,
                count: n - i - 1,
            });
        }
        let first = self.doc.text(tail.trim_start());
        let mut new_inlines = vec![first];
        new_inlines.extend(moved);
        let block = match heading {
            Some(level) => self.doc.heading(level, new_inlines),
            None => self.doc.paragraph(new_inlines),
        };
        let after = cursor(block.id, vec![Step::Index(0)], 0);
        ops.push(Op::Insert {
            seq: Seq::Blocks,
            index: b + 1,
            items: Items::Blocks(vec![block]),
        });
        self.commit(ops, after, Last::Other)
    }

    /// Make the caret's paragraph or heading a heading of `level`, or a
    /// paragraph with `None`. Returns false outside text blocks.
    pub fn set_heading(&mut self, level: Option<u8>) -> bool {
        self.finish_word();
        self.selected = None;
        let Some(b) = self.doc.block_index(self.caret.block) else {
            return false;
        };
        let current = match &self.doc.blocks[b].kind {
            Block::Heading { level, .. } => Some(*level),
            Block::Paragraph(_) => None,
            _ => return false,
        };
        if current == level {
            return false;
        }
        let after = self.caret.clone();
        self.retype(b, level, 0, after)
    }

    /// Replace text block `b` by a heading/paragraph with the same inlines
    /// (and ids), minus the first `strip` chars.
    fn retype(&mut self, b: usize, level: Option<u8>, strip: usize, after: CursorPath) -> bool {
        let block = &self.doc.blocks[b];
        let Some(inlines) = block.kind.inlines() else {
            return false;
        };
        let mut inlines = inlines.clone();
        if let Some(InlineNode {
            kind: Inline::Text(t),
            ..
        }) = inlines.first_mut()
        {
            *t = t
                .chars()
                .skip(strip)
                .collect::<String>()
                .trim_start()
                .into();
        }
        let kind = match level {
            Some(level) => Block::Heading {
                level: level.clamp(1, 6),
                inlines,
            },
            None => Block::Paragraph(inlines),
        };
        let op = Op::Replace {
            seq: Seq::Blocks,
            index: b,
            count: 1,
            items: Items::Blocks(vec![BlockNode { id: block.id, kind }]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    // ----- Moving -----

    /// Move the caret one position left, entering and leaving structures.
    pub fn move_left(&mut self) -> bool {
        self.finish_word();
        let next = self.neighbour(true);
        next.is_some_and(|c| self.jump(c))
    }

    pub fn move_right(&mut self) -> bool {
        self.finish_word();
        let next = self.neighbour(false);
        next.is_some_and(|c| self.jump(c))
    }

    fn neighbour(&self, back: bool) -> Option<CursorPath> {
        let b = self.doc.block_index(self.caret.block)?;
        let list = self.doc.positions(&self.doc.blocks[b]);
        if let Some(i) = list.iter().position(|c| *c == self.caret) {
            let next = if back { i.checked_sub(1) } else { Some(i + 1) };
            if let Some(c) = next.and_then(|j| list.get(j)) {
                return Some(c.clone());
            }
        }
        if back {
            (b > 0).then(|| self.doc.block_end(b - 1))
        } else {
            (b + 1 < self.doc.blocks.len()).then(|| self.doc.block_start(b + 1))
        }
    }

    /// Tab / Shift+Tab: the next (previous) empty slot of the block.
    pub fn next_slot(&mut self, back: bool) -> bool {
        self.finish_word();
        let Some(b) = self.doc.block_index(self.caret.block) else {
            return false;
        };
        let list = self.doc.positions(&self.doc.blocks[b]);
        let Some(i) = list.iter().position(|c| *c == self.caret) else {
            return false;
        };
        let is_slot = |c: &&CursorPath| matches!(self.doc.resolve(c), Some(Target::Row { row }) if row.nodes.is_empty());
        let found = if back {
            list[..i].iter().rev().find(is_slot)
        } else {
            list[i + 1..].iter().find(is_slot)
        };
        match found.cloned() {
            Some(c) => self.jump(c),
            None => false,
        }
    }

    /// ↑/↓ inside a structure: the row above/below in the innermost
    /// fraction or scripts that has one (numerator ↔ denominator, base ↔
    /// scripts). The offset is 0; the widget picks the column. `None` means
    /// move by lines.
    pub fn vertical_target(&self, down: bool) -> Option<CursorPath> {
        let p = self.row_pos()?;
        let mut path = p.path.clone();
        while path.len() > p.base {
            let n = path.len();
            let Step::Index(j) = path[n - 2] else {
                return None;
            };
            let step = path[n - 1];
            let parent = &path[..n - 2];
            let prow = self.row_id_at(p.block, parent)?;
            let node = self.row_nodes(prow).get(j)?;
            let to = match (step, down) {
                (Step::Num, true) => Some(Step::Den),
                (Step::Den, false) => Some(Step::Num),
                (Step::Sup, true) | (Step::Sub, false) => Some(Step::Base),
                (Step::Base, true) => Some(Step::Sub),
                (Step::Base, false) => Some(Step::Sup),
                _ => None,
            };
            if let Some(to) = to.filter(|t| node.child(*t).is_some()) {
                return Some(cursor(p.block, child_path(parent, j, to), 0));
            }
            path.truncate(n - 2);
        }
        None
    }

    // ----- Status and highlights -----

    /// Select the structure of breadcrumb entry `path_len` (see
    /// [`Document::breadcrumb`]): a formula, a structure or a whole row.
    pub fn select_crumb(&mut self, path_len: usize) -> bool {
        self.finish_word();
        let c = self.caret.clone();
        let Some(block) = self.doc.block(c.block) else {
            return false;
        };
        let inline_formula = block.kind.inlines().is_some();
        let prefix = &c.path[..path_len.min(c.path.len())];
        let sel = match prefix {
            [] => return false,
            [Step::Index(i)] if inline_formula => Selected::Formula {
                block: c.block,
                inline: *i,
            },
            [.., Step::Index(k)] => Selected::Nodes {
                block: c.block,
                path: prefix[..prefix.len() - 1].to_vec(),
                start: *k,
                end: k + 1,
            },
            _ => {
                let Some(row) = self.row_id_at(c.block, prefix) else {
                    return false;
                };
                let len = self.row_nodes(row).len();
                if len == 0 {
                    return false;
                }
                Selected::Nodes {
                    block: c.block,
                    path: prefix.to_vec(),
                    start: 0,
                    end: len,
                }
            }
        };
        self.last = Last::Other;
        self.selected = Some(sel);
        true
    }

    /// Ids of the selected nodes (or the selected inline formula).
    pub fn selected_ids(&self) -> Vec<NodeId> {
        match &self.selected {
            Some(Selected::Nodes {
                block,
                path,
                start,
                end,
            }) => self
                .row_id_at(*block, path)
                .map(|r| self.row_nodes(r))
                .and_then(|n| n.get(*start..*end))
                .map_or(vec![], |n| n.iter().map(|n| n.id).collect()),
            Some(Selected::Formula { block, inline }) => self
                .doc
                .block(*block)
                .and_then(|b| b.kind.inlines())
                .and_then(|v| v.get(*inline))
                .map_or(vec![], |i| vec![i.id]),
            None => vec![],
        }
    }

    /// The formula around the caret and the structures it is in, outermost
    /// first (empty in text).
    pub fn context_ids(&self) -> Vec<NodeId> {
        let Some(p) = self.row_pos() else {
            return vec![];
        };
        let Some(block) = self.doc.block(p.block) else {
            return vec![];
        };
        let (mut row, formula) = match (&block.kind, p.path.first()) {
            (Block::DisplayMath(row), _) => (row, row.id),
            (Block::Paragraph(v) | Block::Heading { inlines: v, .. }, Some(Step::Index(i))) => {
                match v.get(*i).map(|n| (&n.kind, n.id)) {
                    Some((Inline::Math(row), id)) => (row, id),
                    _ => return vec![],
                }
            }
            _ => return vec![],
        };
        let mut out = vec![formula];
        for pair in p.path[p.base..].chunks(2) {
            let [Step::Index(k), step] = pair else {
                break;
            };
            let Some(node) = row.nodes.get(*k) else {
                break;
            };
            out.push(node.id);
            match node.child(*step) {
                Some(r) => row = r,
                None => break,
            }
        }
        out
    }

    /// A short key hint for the status bar.
    pub fn hint(&self) -> &'static str {
        match (&self.selected, self.row_pos()) {
            (Some(_), _) => "Backspace → delete selection · Esc → cancel",
            (None, Some(p)) if p.nested() => "Tab → next slot · Esc → leave structure",
            (None, Some(_)) => "/ fraction · ^ _ scripts · Esc or $ → leave formula",
            (None, None) => "$ → formula · = and space at the start → section",
        }
    }
}

fn is_empty_text_block(block: &BlockNode) -> bool {
    matches!(
        block.kind.inlines().map(|v| v.as_slice()),
        Some([InlineNode { kind: Inline::Text(s), .. }]) if s.is_empty()
    )
}

fn delete_block(index: usize) -> Op {
    Op::Delete {
        seq: Seq::Blocks,
        index,
        count: 1,
    }
}

fn text_of(inline: &InlineNode) -> &str {
    match &inline.kind {
        Inline::Text(s) => s,
        Inline::Math(_) => "",
    }
}

/// The text to append to `head` when joining it with `tail`, without
/// doubling a space at the seam or leading the paragraph with one.
fn joined_tail(head: &str, tail: &str, head_starts_block: bool) -> String {
    if head.ends_with(char::is_whitespace) || head.is_empty() && head_starts_block {
        tail.trim_start().into()
    } else {
        tail.into()
    }
}

/// Remove formula `m` of a paragraph and join the texts on either side.
fn remove_formula(block: crate::NodeId, inlines: &[InlineNode], m: usize) -> (Vec<Op>, CursorPath) {
    let before = &inlines[m - 1];
    let after = &inlines[m + 1];
    let len = text_of(before).chars().count();
    let ops = vec![
        Op::Delete {
            seq: Seq::Inlines(block),
            index: m,
            count: 2,
        },
        Op::Insert {
            seq: Seq::Text(before.id),
            index: len,
            items: Items::Text(joined_tail(text_of(before), text_of(after), m == 1)),
        },
    ];
    let caret = CursorPath {
        block,
        path: vec![Step::Index(m - 1)],
        offset: len,
    };
    (ops, caret)
}

/// Append text block `first + 1` to text block `first`.
fn join_blocks(doc: &Document, first: usize) -> (Vec<Op>, CursorPath) {
    let a = &doc.blocks[first];
    let b = &doc.blocks[first + 1];
    let (ai, bi) = (
        a.kind.inlines().expect("text block"),
        b.kind.inlines().expect("text block"),
    );
    let last = ai.last().expect("paragraph invariant");
    let len = text_of(last).chars().count();
    let mut ops = vec![
        // Remove the second block first so its nodes never exist twice.
        delete_block(first + 1),
        Op::Insert {
            seq: Seq::Text(last.id),
            index: len,
            items: Items::Text(joined_tail(text_of(last), text_of(&bi[0]), ai.len() == 1)),
        },
    ];
    if bi.len() > 1 {
        ops.push(Op::Insert {
            seq: Seq::Inlines(a.id),
            index: ai.len(),
            items: Items::Inlines(bi[1..].to_vec()),
        });
    }
    let caret = CursorPath {
        block: a.id,
        path: vec![Step::Index(ai.len() - 1)],
        offset: len,
    };
    (ops, caret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::MathKind;

    /// "ab $x$ cd" / "ef" / display "y" / "gh"
    fn sample() -> Editor {
        let mut d = Document::empty();
        let (t1, x, t2) = (d.text("ab "), d.ident("x"), d.text(" cd"));
        let m = d.math(vec![x]);
        let p1 = d.paragraph(vec![t1, m, t2]);
        let t = d.text("ef");
        let p2 = d.paragraph(vec![t]);
        let y = d.ident("y");
        let disp = d.display(vec![y]);
        let t = d.text("gh");
        let p3 = d.paragraph(vec![t]);
        d.blocks = vec![p1, p2, disp, p3];
        Editor::new(d)
    }

    fn texts(e: &Editor) -> Vec<String> {
        e.doc()
            .blocks
            .iter()
            .map(|b| match &b.kind {
                Block::Paragraph(v) | Block::Heading { inlines: v, .. } => v
                    .iter()
                    .map(|i| match &i.kind {
                        Inline::Text(s) => s.clone(),
                        Inline::Math(_) => "$".into(),
                    })
                    .collect(),
                Block::DisplayMath(_) => "$$".into(),
                Block::RawTypst(s) => s.clone(),
            })
            .collect()
    }

    fn caret_at(e: &mut Editor, block: usize, inline: usize, offset: usize) {
        let id = e.doc().blocks[block].id;
        e.set_caret(CursorPath {
            block: id,
            path: vec![Step::Index(inline)],
            offset,
        });
    }

    #[test]
    fn typing_groups_words_for_undo() {
        let mut e = sample();
        caret_at(&mut e, 1, 0, 2);
        for c in "xy zw".chars() {
            assert!(e.insert_text(&c.to_string()));
        }
        assert_eq!(texts(&e)[1], "efxy zw");
        assert_eq!(e.caret().offset, 7);
        e.undo();
        assert_eq!(texts(&e)[1], "efxy ");
        assert_eq!(e.caret().offset, 5);
        e.undo();
        assert_eq!(texts(&e)[1], "ef");
        assert_eq!(e.caret().offset, 2);
        e.redo();
        e.redo();
        assert_eq!(texts(&e)[1], "efxy zw");
        // A caret move starts a new group.
        e.move_left();
        e.insert_text("Q");
        e.undo();
        assert_eq!(texts(&e)[1], "efxy zw");
    }

    #[test]
    fn extra_spaces_are_not_inserted() {
        let mut e = sample();
        caret_at(&mut e, 1, 0, 2);
        e.insert_text(" ");
        assert!(!e.insert_text(" "), "second space ignored");
        e.insert_text("x  \t y");
        assert_eq!(texts(&e)[1], "ef x y");
        // Before an existing space, a typed space is dropped too.
        caret_at(&mut e, 1, 0, 2);
        assert!(!e.insert_text(" "));
        // No space at the start of a paragraph.
        caret_at(&mut e, 1, 0, 0);
        assert!(!e.insert_text(" "));
        assert_eq!(e.caret().offset, 0);
    }

    #[test]
    fn deleting_never_leaves_double_spaces() {
        let mut e = sample();
        caret_at(&mut e, 1, 0, 2);
        e.insert_text(" b c");
        caret_at(&mut e, 1, 0, 4); // after "b"
        e.backspace();
        assert_eq!(texts(&e)[1], "ef c");
        assert_eq!(e.caret().offset, 2);
        caret_at(&mut e, 1, 0, 0);
        e.delete();
        e.delete();
        assert_eq!(texts(&e)[1], "c", "no leading space");
        // Removing a formula between two spaces keeps one.
        caret_at(&mut e, 0, 2, 0);
        e.backspace();
        e.backspace();
        assert_eq!(texts(&e)[0], "ab cd");
    }

    #[test]
    fn backspace_and_delete_chars() {
        let mut e = sample();
        caret_at(&mut e, 1, 0, 1);
        assert!(e.backspace());
        assert_eq!(texts(&e)[1], "f");
        assert!(!e.doc().blocks.is_empty());
        assert!(e.delete());
        assert_eq!(texts(&e)[1], "");
        e.undo();
        assert_eq!(texts(&e)[1], "ef");
    }

    #[test]
    fn backspace_removes_formula_and_joins_text() {
        let mut e = sample();
        caret_at(&mut e, 0, 2, 0);
        assert!(e.backspace());
        assert_eq!(texts(&e)[0], "ab $ cd", "first press selects");
        assert!(e.selected().is_some());
        assert!(e.backspace());
        assert_eq!(texts(&e)[0], "ab cd");
        assert_eq!(e.caret().path, vec![Step::Index(0)]);
        assert_eq!(e.caret().offset, 3);
        e.undo();
        assert_eq!(texts(&e)[0], "ab $ cd");
        assert!(e.doc().check_invariants().is_ok());
    }

    #[test]
    fn enter_splits_and_backspace_joins() {
        let mut e = sample();
        caret_at(&mut e, 0, 0, 1);
        assert!(e.enter());
        assert_eq!(texts(&e)[..2], ["a", "b $ cd"]);
        assert_eq!(e.caret().block, e.doc().blocks[1].id);
        assert_eq!(e.caret().offset, 0);
        assert!(e.backspace());
        assert_eq!(texts(&e)[..2], ["ab $ cd", "ef"]);
        assert_eq!(e.caret().offset, 1);
        // Delete at the end of a paragraph joins the next one.
        let n = e.doc().blocks[0].kind.inlines().unwrap().len();
        caret_at(&mut e, 0, n - 1, 3);
        assert!(e.delete());
        assert_eq!(texts(&e)[0], "ab $ cdef");
        e.undo();
        e.undo();
        e.undo();
        assert_eq!(texts(&e)[..2], ["ab $ cd", "ef"]);
        assert!(e.doc().check_invariants().is_ok());
    }

    #[test]
    fn equals_space_makes_headings() {
        let mut e = sample();
        caret_at(&mut e, 1, 0, 0);
        e.insert_text("== ");
        assert!(matches!(
            e.doc().blocks[1].kind,
            Block::Heading { level: 2, .. }
        ));
        assert_eq!(texts(&e)[1], "ef");
        assert_eq!(e.caret().offset, 0);
        // Backspace at the start: back to a paragraph, then join.
        e.backspace();
        assert!(matches!(e.doc().blocks[1].kind, Block::Paragraph(_)));
        e.backspace();
        assert_eq!(texts(&e)[0], "ab $ cdef");
        while e.undo() {}
        assert_eq!(texts(&e)[1], "ef");
        // `=` elsewhere is just text; shortcuts set the level.
        caret_at(&mut e, 1, 0, 1);
        e.insert_text("= ");
        assert_eq!(texts(&e)[1], "e= f");
        assert!(e.set_heading(Some(3)));
        assert!(matches!(
            e.doc().blocks[1].kind,
            Block::Heading { level: 3, .. }
        ));
        assert!(e.set_heading(None));
        assert!(!e.set_heading(None));
    }

    #[test]
    fn enter_at_end_of_heading_starts_paragraph() {
        let mut d = Document::empty();
        let t = d.text("Title");
        let h = d.heading(1, vec![t]);
        d.blocks = vec![h];
        let mut e = Editor::new(d);
        caret_at(&mut e, 0, 0, 5);
        e.enter();
        assert!(matches!(e.doc().blocks[1].kind, Block::Paragraph(_)));
        caret_at(&mut e, 0, 0, 2);
        e.enter();
        assert!(matches!(e.doc().blocks[1].kind, Block::Heading { .. }));
    }

    #[test]
    fn arrows_enter_formulas_and_cross_blocks() {
        let mut e = sample();
        caret_at(&mut e, 0, 0, 2);
        let mut seen = vec![];
        for _ in 0..5 {
            e.move_right();
            seen.push((e.caret().path.clone(), e.caret().offset));
        }
        let i = Step::Index;
        assert_eq!(
            seen,
            [
                (vec![i(0)], 3),
                (vec![i(1)], 0), // inside the formula, before x
                (vec![i(1)], 1),
                (vec![i(2)], 0), // after the formula
                (vec![i(2)], 1),
            ]
        );
        for _ in 0..4 {
            e.move_left();
        }
        assert_eq!((e.caret().path.clone(), e.caret().offset), (vec![i(0)], 3));
        // Across blocks: end of "ef" → display row → "gh".
        caret_at(&mut e, 1, 0, 2);
        e.move_right();
        assert_eq!(e.caret().block, e.doc().blocks[2].id);
        assert_eq!(e.caret().offset, 0);
        e.move_right();
        e.move_right();
        assert_eq!(e.caret().block, e.doc().blocks[3].id);
        e.move_left();
        assert_eq!(e.caret().block, e.doc().blocks[2].id);
        assert_eq!(e.caret().offset, 1);
    }

    #[test]
    fn empty_paragraph_before_formula_is_removed() {
        let mut e = sample();
        caret_at(&mut e, 1, 0, 2);
        e.backspace();
        e.backspace();
        // Now "ef" is empty; Delete removes the empty block before the formula.
        assert!(e.delete());
        assert_eq!(texts(&e), ["ab $ cd", "$$", "gh"]);
        // Backspace at the start of "gh" goes into the formula block.
        caret_at(&mut e, 2, 0, 0);
        assert!(e.backspace());
        assert_eq!(e.caret().block, e.doc().blocks[1].id);
    }

    /// A compact rendering of a row for assertions.
    fn show(row: &crate::tree::MathRow) -> String {
        row.nodes
            .iter()
            .map(|n| match &n.kind {
                MathKind::Sym(s) | MathKind::Ident(s) | MathKind::Num(s) => s.clone(),
                MathKind::Slot => "□".into(),
                MathKind::Frac { num, den } => format!("frac({}, {})", show(num), show(den)),
                MathKind::Scripts { base, sup, sub } => format!(
                    "{}{}{}",
                    show(base),
                    sup.as_ref()
                        .map_or(String::new(), |r| format!("^({})", show(r))),
                    sub.as_ref()
                        .map_or(String::new(), |r| format!("_({})", show(r))),
                ),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The display formula of block `b`.
    fn display_of(e: &Editor, b: usize) -> String {
        match &e.doc().blocks[b].kind {
            Block::DisplayMath(row) => show(row),
            _ => panic!("not display math"),
        }
    }

    fn inline_of(e: &Editor, b: usize, i: usize) -> String {
        match &e.doc().blocks[b].kind.inlines().unwrap()[i].kind {
            Inline::Math(row) => show(row),
            _ => panic!("not a formula"),
        }
    }

    fn empty_display() -> Editor {
        let mut d = Document::empty();
        let b = d.display(vec![]);
        d.blocks = vec![b];
        Editor::new(d)
    }

    fn labels(e: &Editor) -> Vec<&'static str> {
        e.doc()
            .breadcrumb(e.caret())
            .iter()
            .map(|c| c.label)
            .collect()
    }

    /// The acceptance sequence of the math milestone, key by key.
    #[test]
    fn types_the_entropy_bound_structurally() {
        let mut e = empty_display();
        e.insert_text("S <= (log chi)");
        assert_eq!(display_of(&e, 0), "S ≤ ( log χ )");
        e.insert_text("/");
        assert_eq!(display_of(&e, 0), "S ≤ frac(log χ, )");
        assert_eq!(labels(&e), ["display math", "fraction", "denominator"]);
        e.insert_text("(2");
        assert_eq!(labels(&e), ["display math", "fraction", "denominator"]);
        e.insert_text(")");
        assert_eq!(labels(&e), ["display math"]);
        assert_eq!(e.caret().offset, 3, "just after the fraction");
        e.insert_text(" + c");
        assert_eq!(display_of(&e, 0), "S ≤ frac(log χ, 2) + c");
        // One undo step per structural action; all of it undoes cleanly.
        while e.undo() {}
        assert_eq!(display_of(&e, 0), "");
    }

    #[test]
    fn same_formula_with_slots_and_arrows() {
        let mut e = empty_display();
        e.insert_text("S<=/");
        assert_eq!(labels(&e), ["display math", "fraction", "numerator"]);
        e.insert_text("log chi");
        let down = e.vertical_target(true).unwrap();
        e.set_caret(down);
        e.insert_text("2");
        e.move_right(); // leaves the fraction
        e.insert_text("+c");
        assert_eq!(display_of(&e, 0), "S ≤ frac(log χ, 2) + c");
    }

    #[test]
    fn dollar_makes_inline_and_display_formulas() {
        let mut e = sample();
        caret_at(&mut e, 1, 0, 1);
        e.insert_text("$x^2");
        assert_eq!(texts(&e)[1], "e$f");
        assert_eq!(inline_of(&e, 1, 1), "x^(2)");
        e.insert_text("$");
        assert_eq!(e.caret().path, vec![Step::Index(2)], "in the text after");
        assert_eq!(e.caret().offset, 0);
        e.insert_text(" g");
        assert_eq!(texts(&e)[1], "e$ gf");

        let mut e = sample();
        caret_at(&mut e, 1, 0, 1);
        e.insert_text("$$");
        assert_eq!(texts(&e)[1..4], ["e", "$$", "f"]);
        e.insert_text("a");
        assert_eq!(display_of(&e, 2), "a");
    }

    #[test]
    fn scripts_and_slot_removal() {
        let mut e = empty_display();
        e.insert_text("x^2");
        assert_eq!(display_of(&e, 0), "x^(2)");
        e.escape();
        e.insert_text("_");
        assert_eq!(display_of(&e, 0), "x^(2)_()");
        e.insert_text("i");
        assert_eq!(display_of(&e, 0), "x^(2)_(i)");
        e.backspace(); // deletes i
        e.backspace(); // empty subscript slot: removed, caret at end of base
        assert_eq!(display_of(&e, 0), "x^(2)");
        assert_eq!(labels(&e), ["display math", "scripts", "base"]);
        e.escape();
        e.insert_text("/");
        assert_eq!(display_of(&e, 0), "frac(x^(2), )");
        e.backspace(); // empty denominator: unwrap to the numerator
        assert_eq!(display_of(&e, 0), "x^(2)");
        assert_eq!(e.caret().path, Vec::<Step>::new());
        assert_eq!(e.caret().offset, 1);
    }

    #[test]
    fn backspace_selects_structures_first() {
        let mut e = empty_display();
        e.insert_text("a+(b)/c");
        e.escape();
        assert_eq!(display_of(&e, 0), "a + frac(b, c)");
        assert!(e.backspace());
        assert_eq!(display_of(&e, 0), "a + frac(b, c)");
        assert_eq!(e.selected_ids().len(), 1);
        assert!(e.backspace());
        assert_eq!(display_of(&e, 0), "a +");
        e.undo();
        assert_eq!(display_of(&e, 0), "a + frac(b, c)");
    }

    #[test]
    fn words_symbols_and_numbers() {
        let mut e = empty_display();
        e.insert_text("\\alpha + beta2.50 -> sin x != \\leq ");
        assert_eq!(display_of(&e, 0), "α + β 2.50 → sin x ≠ ≤");
        e.insert_text("lo");
        e.backspace();
        e.insert_text("n ");
        assert_eq!(display_of(&e, 0), "α + β 2.50 → sin x ≠ ≤ ln");
    }

    /// A stand-in for the Typst resolver.
    fn fake_resolver(name: &str) -> Option<String> {
        match name {
            "times" => Some("×".into()),
            "times.o" => Some("⊗".into()),
            "arrow" | "arrow.r" => Some("→".into()),
            "arrow.r.long" => Some("⟶".into()),
            "chi" => Some("χ".into()),
            _ => None,
        }
    }

    #[test]
    fn dotted_symbol_names() {
        let mut e = empty_display();
        e.set_symbol_resolver(fake_resolver);
        e.insert_text("a times.o b arrow.r.long c times b");
        assert_eq!(display_of(&e, 0), "a ⊗ b ⟶ c × b");
        // Not symbols: the dot stays a dot.
        let mut e = empty_display();
        e.set_symbol_resolver(fake_resolver);
        e.insert_text("x. times.q chi. ");
        assert_eq!(display_of(&e, 0), "x . × . q χ .");
        e.insert_text("\\times.o ");
        assert_eq!(display_of(&e, 0), "x . × . q χ . ⊗");
        while e.undo() {}
        assert_eq!(display_of(&e, 0), "");
    }

    #[test]
    fn tab_visits_empty_slots_and_escape_leaves() {
        let mut e = empty_display();
        e.insert_text("/");
        e.escape();
        e.insert_text("=/");
        e.escape();
        assert_eq!(display_of(&e, 0), "frac(, ) = frac(, )");
        let start = e.doc().block_start(0);
        e.set_caret(start);
        let mut visits = vec![];
        while e.next_slot(false) {
            visits.push(labels(&e).last().copied().unwrap());
        }
        assert_eq!(
            visits,
            ["numerator", "denominator", "numerator", "denominator"]
        );
        e.next_slot(true);
        assert_eq!(labels(&e).last(), Some(&"numerator"));
        e.escape();
        assert_eq!(e.caret().offset, 3, "after the second fraction");
        assert!(e.escape(), "leaving the display formula");
        assert!(matches!(e.doc().blocks[1].kind, Block::Paragraph(_)));
    }

    #[test]
    fn breadcrumb_selects_structures() {
        let mut e = empty_display();
        e.insert_text("a/b");
        let crumbs = e.doc().breadcrumb(e.caret());
        assert!(e.select_crumb(crumbs[1].path_len)); // the fraction
        assert_eq!(e.selected_ids().len(), 1);
        e.backspace();
        assert_eq!(display_of(&e, 0), "");
        let mut e = sample();
        caret_at(&mut e, 0, 1, 1);
        assert_eq!(e.context_ids().len(), 1);
        assert!(e.select_crumb(1));
        assert!(matches!(
            e.selected(),
            Some(Selected::Formula { inline: 1, .. })
        ));
    }
}
