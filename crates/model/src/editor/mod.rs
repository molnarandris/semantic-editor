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
use crate::tree::{
    Block, BlockNode, Document, Inline, InlineNode, MathKind, MathNode, NodeId, Place,
};

mod delete;
mod input;
mod motion;
mod structure;
#[cfg(test)]
mod tests;

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
    /// Editing the source of a raw or diagram block in the side pane.
    Source(NodeId),
    /// Editing the document header.
    Header,
}

/// A block inserted by [`Editor::insert_block`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewBlock {
    Diagram,
    RawTypst,
}

/// The source a new diagram starts with.
pub const DIAGRAM_TEMPLATE: &str = "circle((0, 0), radius: 1)
line((0, 0), (1, 0), mark: (end: \">\"))
content((1.4, 0), $x$)";

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
    /// A whole raw Typst or diagram block.
    Block { block: NodeId },
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
    place: Place,
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
    place: Place,
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

fn is_sym(node: &MathNode, s: &str) -> bool {
    matches!(&node.kind, MathKind::Sym(x) if x == s)
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(Document::new())
    }
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        let caret = doc.doc_start();
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
        let place = self.doc.place(c.block)?;
        let block = self.doc.at(place)?;
        let inlines = block.kind.inlines()?;
        let [Step::Index(inline)] = c.path[..] else {
            return None;
        };
        let Inline::Text(text) = &inlines.get(inline)?.kind else {
            return None;
        };
        Some(TextPos {
            place,
            block,
            inlines,
            inline,
            offset: c.offset,
            len: text.chars().count(),
        })
    }

    fn row_pos(&self) -> Option<RowPos> {
        let c = &self.caret;
        let place = self.doc.place(c.block)?;
        let block = self.doc.at(place)?;
        let Ok(Target::Row { row }) = self.doc.walk(block, &c.path) else {
            return None;
        };
        Some(RowPos {
            place,
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
            (Last::Source(a), Last::Source(b)) => a == b,
            (Last::Header, Last::Header) => true,
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

    /// Select a subtree (the first Backspace/Delete on a structure, a
    /// breadcrumb click). The caret stays.
    fn select(&mut self, sel: Selected) -> bool {
        self.last = Last::Other;
        self.selected = Some(sel);
        true
    }

    /// Select node `k` of the caret's row.
    fn select_node(&mut self, p: &RowPos, k: usize) -> bool {
        self.select(Selected::Nodes {
            block: p.block,
            path: p.path.clone(),
            start: k,
            end: k + 1,
        })
    }

    /// The node just before the caret in its row.
    fn node_before(&self, p: &RowPos) -> Option<MathNode> {
        let k = p.offset.checked_sub(1)?;
        self.row_nodes(p.row).get(k).cloned()
    }

    /// Insert `node` at the caret, caret after it (one typing step).
    fn type_node(&mut self, p: &RowPos, node: MathNode) -> bool {
        let op = row_insert(p.row, p.offset, node);
        self.commit(vec![op], p.at(p.offset + 1), Last::Typing { space: false })
    }

    /// Replace the node before the caret by `kind`, keeping its id (a word
    /// or number growing, `<` `=` becoming ≤).
    fn retype_node_before(&mut self, p: &RowPos, id: NodeId, kind: MathKind) -> bool {
        let op = row_replace(p.row, p.offset - 1, MathNode { id, kind });
        self.commit(vec![op], p.at(p.offset), Last::Typing { space: false })
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
        self.select(sel)
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
            Some(Selected::Block { block }) => vec![*block],
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
            (None, _) if self.source_block().is_some() => {
                "Ctrl+Enter or double click → edit source · Enter → new paragraph · Backspace → delete"
            }
            (None, Some(p)) if p.nested() => "Tab → next slot · Esc → leave structure",
            (None, Some(_)) => "/ fraction · ^ _ scripts · Esc or $ → leave formula",
            (None, None) if self.typing_env_name() => "space → environment (e.g. #lemma, #proof)",
            (None, None) if self.in_env() => {
                "Esc → leave environment · Enter on an empty last paragraph → leave"
            }
            (None, None) => {
                "$ → formula · = and space at the start → section · #name and space → environment"
            }
        }
    }

    /// Whether the caret is inside an environment.
    fn in_env(&self) -> bool {
        self.doc
            .place(self.caret.block)
            .is_some_and(|p| p.parent.is_some())
    }

    /// Whether the caret's paragraph is `#name` so far, with the caret at
    /// its end (space would make it an environment).
    fn typing_env_name(&self) -> bool {
        self.text_pos().is_some_and(|pos| {
            pos.inlines.len() == 1
                && pos.offset == pos.len
                && matches!(pos.block.kind, Block::Paragraph(_))
                && input::env_name(&pos.chars()).is_some()
        })
    }

    /// Start an environment: a paragraph with `#` (after the caret's block,
    /// or the caret's empty paragraph itself) for the name to be typed.
    pub fn start_env(&mut self) -> bool {
        self.finish_word();
        self.selected = None;
        let empty_here = self.text_pos().is_some_and(|pos| {
            matches!(pos.block.kind, Block::Paragraph(_)) && is_empty_text_block(pos.block)
        });
        if !empty_here {
            let Some(place) = self.doc.place(self.caret.block) else {
                return false;
            };
            if !self.paragraph_after(place) {
                return false;
            }
        }
        self.insert_text("#")
    }
}

fn is_empty_text_block(block: &BlockNode) -> bool {
    matches!(
        block.kind.inlines().map(|v| v.as_slice()),
        Some([InlineNode { kind: Inline::Text(s), .. }]) if s.is_empty()
    )
}

fn delete_block(place: Place) -> Op {
    Op::Delete {
        seq: place.seq(),
        index: place.index,
        count: 1,
    }
}

fn text_of(inline: &InlineNode) -> &str {
    match &inline.kind {
        Inline::Text(s) => s,
        Inline::Math(_) => "",
    }
}
