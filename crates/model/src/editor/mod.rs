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
    Block, BlockNode, Document, Inline, InlineNode, ListKind, MathKind, MathNode, NodeId, Place,
};

mod delete;
use delete::joined_tail;
mod input;
mod list;
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

/// The code a new inline diagram (in a formula) starts with.
pub const INLINE_DIAGRAM_TEMPLATE: &str = "circle((0, 0), radius: 0.5)
line((-0.5, 0), (0.5, 0))";

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
    /// An inline formula or call: `path` leads to it (ends with its
    /// `Index`).
    Inline { block: NodeId, path: Vec<Step> },
    /// A whole block (raw Typst, diagram, environment).
    Block { block: NodeId },
    /// An environment marked by Backspace at the start of its text: its
    /// outline is drawn stronger, and the next Backspace unwraps it.
    Unwrap { env: NodeId },
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
    /// What the functions called as blocks take, provided by the GUI from
    /// the Typst bridge. Without it, every name may take a title.
    calls: Option<CallResolver>,
    /// What functions called in math take, from the Typst bridge. Without
    /// it, nothing typed in math becomes a call.
    math_calls: Option<MathCallResolver>,
}

/// Looks up a symbol name with `.modifiers`; `None` if it is not a symbol.
pub type SymbolResolver = fn(&str) -> Option<String>;

/// What a function called as a block (`#name[…]`) takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallShape {
    /// A content argument: the body.
    pub body: bool,
    /// Another content argument before it: a title, `#name[Title][…]`.
    pub title: bool,
}

/// What a function called in math takes: `slots` content arguments to
/// start with; `variadic` ones (`cases`) take more, one per comma.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MathCallShape {
    pub slots: usize,
    pub variadic: bool,
}

/// The shape of `name` called in math given the sources defining things
/// (as for [`CallResolver`]); `None` if it is not a function to call there.
pub type MathCallResolver = fn(&[&str], &str) -> Option<MathCallShape>;

/// The shape of function `name` given the Typst sources defining things
/// (the header, then the raw blocks in document order); `None` if unknown.
pub type CallResolver = fn(&[&str], &str) -> Option<CallShape>;

/// Position of a text caret inside a paragraph or heading (or a title,
/// a term, the body of an inline call in them).
struct TextPos<'a> {
    place: Place,
    block: &'a BlockNode,
    /// The block, or the inline call whose body holds the text.
    owner: NodeId,
    /// The path to the inline list (empty, or into calls' bodies).
    prefix: Vec<Step>,
    inlines: &'a [InlineNode],
    inline: usize,
    offset: usize,
    len: usize,
}

impl TextPos<'_> {
    fn chars(&self) -> Vec<char> {
        text_of(&self.inlines[self.inline]).chars().collect()
    }

    /// Inside an inline call's body.
    fn nested(&self) -> bool {
        !self.prefix.is_empty()
    }

    /// The caret at `offset` in inline `inline` of the same list.
    fn at(&self, inline: usize, offset: usize) -> CursorPath {
        let mut path = self.prefix.clone();
        path.push(Step::Index(inline));
        cursor(self.block.id, path, offset)
    }
}

/// Position of a caret in a math row.
#[derive(Clone, Debug)]
struct RowPos {
    place: Place,
    block: NodeId,
    /// Length of the path prefix before the formula's row path (0 for
    /// display math; for an inline formula, the path to it, ending with its
    /// `Index`).
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

    /// An inline formula's index in its list and the path to the list.
    fn formula(&self) -> Option<(Vec<Step>, usize)> {
        let k = self.base.checked_sub(1)?;
        let Step::Index(i) = self.path[k] else {
            return None;
        };
        Some((self.path[..k].to_vec(), i))
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
            calls: None,
            math_calls: None,
        }
    }

    pub fn set_symbol_resolver(&mut self, resolver: SymbolResolver) {
        self.resolver = Some(resolver);
    }

    pub fn set_call_resolver(&mut self, resolver: CallResolver) {
        self.calls = Some(resolver);
    }

    pub fn set_math_call_resolver(&mut self, resolver: MathCallResolver) {
        self.math_calls = Some(resolver);
    }

    /// The header and the raw blocks: the sources that define functions.
    fn defining_sources(&self) -> Vec<&str> {
        let mut sources = vec![self.doc.header.as_str()];
        sources.extend(self.doc.blocks.iter().filter_map(|b| match &b.kind {
            Block::RawTypst(s) => Some(s.as_str()),
            _ => None,
        }));
        sources
    }

    /// What function `name` takes as a block call, if known.
    pub(super) fn call_shape(&self, name: &str) -> Option<CallShape> {
        self.calls?(&self.defining_sources(), name)
    }

    /// What `name` takes called in math, if it is a function there.
    pub(super) fn math_call_shape(&self, name: &str) -> Option<MathCallShape> {
        self.math_calls?(&self.defining_sources(), name)
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
        let old = std::mem::replace(&mut self.caret, self.doc.fix_cursor(&caret));
        self.last = Last::Other;
        self.drop_left_title(&old);
        self.drop_left_call(&old);
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
        let at = self.doc.inline_at(block, &c.path)?;
        if c.path.len() != at.prefix + 1 {
            return None;
        }
        let Inline::Text(text) = &at.inlines.get(at.index)?.kind else {
            return None;
        };
        Some(TextPos {
            place,
            block,
            owner: at.owner,
            prefix: c.path[..at.prefix].to_vec(),
            inlines: at.inlines,
            inline: at.index,
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
        let base = self
            .doc
            .inline_at(block, &c.path)
            .map_or(0, |at| at.prefix + 1);
        Some(RowPos {
            place,
            block: block.id,
            base,
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
                debug_assert_eq!(self.doc.check_invariants(), Ok(()));
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
        let old = std::mem::replace(&mut self.caret, caret);
        self.drop_left_title(&old);
        self.drop_left_call(&old);
        changed
    }

    // ----- Environment titles -----

    /// The environment whose title the caret is in (its text or a formula
    /// in it).
    fn title_env(&self) -> Option<NodeId> {
        let block = self.doc.block(self.caret.block)?;
        (matches!(block.kind, Block::Env { .. }) && !self.caret.path.is_empty()).then_some(block.id)
    }

    /// Whether the caret is at the end of the title (or term) of `owner`,
    /// with every `(` typed in it closed.
    fn at_label_end(&self, owner: NodeId) -> bool {
        let Some(title) = self.doc.block(owner).and_then(|b| b.kind.text_inlines()) else {
            return false;
        };
        if self.caret.block != owner || title.is_empty() {
            return false;
        }
        let text: String = title.iter().map(text_of).collect();
        let open = text.matches('(').count() as isize - text.matches(')').count() as isize;
        let end = title.last().map_or(0, |t| text_of(t).chars().count());
        open <= 0 && self.caret.path == [Step::Index(title.len() - 1)] && self.caret.offset == end
    }

    /// The first caret position of a container's body.
    fn body_start(&self, env: NodeId) -> Option<CursorPath> {
        let first = self.doc.block(env)?.kind.body()?.first()?;
        Some(self.doc.start_of(first))
    }

    /// Put the caret in the title of environment `env` (at its end), adding
    /// an empty title if it has none; an empty title goes again when the
    /// caret leaves it. Not for a function known to take no title.
    pub fn edit_title(&mut self, env: NodeId) -> bool {
        self.finish_word();
        let Some(Block::Env { name, title, .. }) = self.doc.block(env).map(|b| &b.kind) else {
            return false;
        };
        if title.is_empty() && self.call_shape(name).is_some_and(|c| !c.title) {
            return false;
        }
        if let Some(last) = title.last() {
            let end = text_of(last).chars().count();
            return self.jump(cursor(env, vec![Step::Index(title.len() - 1)], end));
        }
        let text = self.doc.text("");
        let op = Op::Insert {
            seq: Seq::Inlines(env),
            index: 0,
            items: Items::Inlines(vec![text]),
        };
        self.commit(vec![op], cursor(env, vec![Step::Index(0)], 0), Last::Other)
    }

    /// Ops that remove the title of environment `env`.
    fn remove_title(&self, env: NodeId) -> Option<Op> {
        let title = match &self.doc.block(env)?.kind {
            Block::Env { title, .. } if !title.is_empty() => title,
            _ => return None,
        };
        Some(Op::Delete {
            seq: Seq::Inlines(env),
            index: 0,
            count: title.len(),
        })
    }

    /// After the caret moved from `old`: if `old` was in an empty title and
    /// the caret is no longer in it, remove the title (its own undo step).
    fn drop_left_title(&mut self, old: &CursorPath) {
        let still_in = old.block == self.caret.block && !self.caret.path.is_empty();
        if old.path.is_empty() || still_in {
            return;
        }
        let empty = match self.doc.block(old.block).map(|b| &b.kind) {
            Some(Block::Env { title, .. }) => is_empty_text(title),
            _ => false,
        };
        if !empty {
            return;
        }
        let Some(op) = self.remove_title(old.block) else {
            return;
        };
        let after = self.caret.clone();
        if self
            .history
            .apply(&mut self.doc, vec![op], old.clone(), after.clone())
            .is_ok()
        {
            self.caret = self.doc.fix_cursor(&after);
        }
    }

    /// After the caret moved from `old`: if `old` was in an empty inline
    /// call (`*` typed, then the caret moved away), remove the call (its own
    /// undo step); the caret keeps its place.
    fn drop_left_call(&mut self, old: &CursorPath) {
        let Some(block) = self.doc.block(old.block) else {
            return;
        };
        let Some(at) = self.doc.inline_at(block, &old.path) else {
            return;
        };
        if at.prefix == 0 || !is_empty_text(at.inlines) {
            return;
        }
        let call = old.path[..at.prefix - 1].to_vec();
        if self.caret.block == old.block && self.caret.path.starts_with(&old.path[..at.prefix]) {
            return;
        }
        // Where the caret is, by the inline it is in (ids survive the edit).
        let here = self.doc.block(self.caret.block).and_then(|b| {
            let at = self.doc.inline_at(b, &self.caret.path)?;
            Some((at.inlines[at.index].id, at.prefix))
        });
        let call_at = self.doc.inline_at(block, &call).expect("the call");
        let m = call_at.index;
        let (before, after) = (&call_at.inlines[m - 1], &call_at.inlines[m + 1]);
        let (before_id, after_id) = (before.id, after.id);
        let before_len = text_of(before).chars().count();
        let starts_block = m == 1 && call.len() == 1;
        let joined = joined_tail(text_of(before), text_of(after), starts_block);
        let trimmed = text_of(after).chars().count() - joined.chars().count();
        let Some((ops, _)) = self.remove_inline(old.block, &call) else {
            return;
        };
        let mut after_caret = self.caret.clone();
        let applied = self
            .history
            .apply(&mut self.doc, ops, old.clone(), after_caret.clone());
        if applied.is_err() {
            return;
        }
        if let Some((id, prefix)) = here {
            let rest = after_caret.path[prefix + 1..].to_vec();
            let (id, shift) = match id == after_id {
                true => (before_id, before_len as isize - trimmed as isize),
                false => (id, 0),
            };
            if let Some(mut path) = self.doc.inline_path(after_caret.block, id) {
                path.extend(rest);
                after_caret.path = path;
                if shift != 0 {
                    after_caret.offset = after_caret.offset.saturating_add_signed(shift);
                }
            }
        }
        self.caret = self.doc.fix_cursor(&after_caret);
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

    /// The code atom or inline diagram to edit: the selected one,
    /// else the one just before the caret, else the one just after it.
    fn code_atom(&self) -> Option<NodeId> {
        let code = |n: &MathNode| n.source().map(|_| n.id);
        if let Some(Selected::Nodes {
            block, path, start, ..
        }) = &self.selected
        {
            let row = self.row_id_at(*block, path)?;
            return self.row_nodes(row).get(*start).and_then(code);
        }
        let p = self.row_pos()?;
        let nodes = self.row_nodes(p.row);
        p.offset
            .checked_sub(1)
            .and_then(|k| nodes.get(k))
            .and_then(code)
            .or_else(|| nodes.get(p.offset).and_then(code))
    }

    /// What Ctrl+Enter or a double click opens in the source dialog: the
    /// caret's raw/diagram block or the code atom next to it.
    pub fn source_target(&self) -> Option<NodeId> {
        self.source_block().or_else(|| self.code_atom())
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
        let prefix = &c.path[..path_len.min(c.path.len())];
        let inline = self
            .doc
            .inline_at(block, prefix)
            .is_some_and(|at| at.prefix + 1 == prefix.len());
        let sel = match prefix {
            [] => return false,
            [.., Step::Index(_)] if inline => Selected::Inline {
                block: c.block,
                path: prefix.to_vec(),
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
            Some(Selected::Inline { block, path }) => self
                .inline_by_path(*block, path)
                .map_or(vec![], |i| vec![i.id]),
            Some(Selected::Block { block }) => vec![*block],
            Some(Selected::Unwrap { .. }) | None => vec![],
        }
    }

    /// The inline (formula, call or text) that `path` in `block` leads to
    /// (`path` ends with its `Index`).
    fn inline_by_path(&self, block: NodeId, path: &[Step]) -> Option<&InlineNode> {
        let block = self.doc.block(block)?;
        let at = self.doc.inline_at(block, path)?;
        (at.prefix + 1 == path.len()).then(|| &at.inlines[at.index])
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
        let (mut row, formula) = match &block.kind {
            Block::DisplayMath(row) => (row, row.id),
            _ => match self.inline_by_path(p.block, &p.path[..p.base]) {
                Some(InlineNode {
                    id,
                    kind: Inline::Math(row),
                }) => (row, *id),
                _ => return vec![],
            },
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
            (Some(Selected::Unwrap { .. }), _) => {
                "Backspace → remove the environment, keep its text · Esc → cancel"
            }
            (Some(_), _) => "Backspace → delete selection · Esc → cancel",
            (_, Some(_)) if self.code_atom().is_some() => {
                "Ctrl+Enter or double click → edit code · Backspace → select, then delete"
            }
            (None, _) if self.source_block().is_some() => {
                "Ctrl+Enter or double click → edit source · Enter → new paragraph · Backspace → delete"
            }
            (None, Some(p)) if self.text_before(&p).is_some() => {
                "text in the formula · \" → end it"
            }
            (None, Some(p)) if self.call_word(&p, true).is_some() => {
                if self.call_word(&p, false).is_some() {
                    "space or ( → function with its arguments"
                } else {
                    "( → function with its arguments · space → symbol"
                }
            }
            (None, Some(p)) if p.nested() => "Tab → next slot · Esc → leave structure",
            (None, Some(_)) => "/ fraction · ^ _ scripts · Esc or $ → leave formula",
            (None, None) if let Some(hint) = self.inline_call_hint() => hint,
            (None, None) if let Some(hint) = self.call_hint() => hint,
            (None, _) if self.term_item().is_some() => "term · : or Enter → on to the description",
            (None, None) if self.item_of(self.caret.block).is_some() => {
                "list item · Enter → next item (on an empty one: out) · Tab / Shift+Tab → indent / outdent"
            }
            (None, _) if self.title_env().is_some() => {
                "title · Enter or ) → on to the text · Esc → just after the environment"
            }
            (None, None) if self.edge().is_some() => {
                "outside the environment: typing or Enter → new paragraph · Backspace → select it"
            }
            (None, None) if self.in_env() => {
                "Esc → just after the environment · click its heading → title · Enter on an empty last paragraph → leave"
            }
            (None, None) => {
                "$ → formula · *bold* _italic_ · = and space → section · - + / and space → list · #name and space → environment"
            }
        }
    }

    /// Whether the caret is inside an environment.
    fn in_env(&self) -> bool {
        self.doc
            .place(self.caret.block)
            .and_then(|p| p.parent)
            .and_then(|p| self.doc.env_around(p))
            .is_some()
    }

    /// The environment to outline: the one the caret is just before/after
    /// (`false`), else the innermost one it is in (`true`).
    pub fn env_focus(&self) -> Option<(NodeId, bool)> {
        if let Some(Selected::Unwrap { env }) = self.selected {
            return Some((env, true));
        }
        if self.edge().is_some() {
            return Some((self.caret.block, false));
        }
        if let Some(env) = self.title_env() {
            return Some((env, true));
        }
        let parent = self.doc.place(self.caret.block)?.parent?;
        Some((self.doc.env_around(parent)?, true))
    }

    /// Whether the next Backspace unwraps the focused environment.
    pub fn env_marked(&self) -> bool {
        matches!(self.selected, Some(Selected::Unwrap { .. }))
    }

    /// The hint in the body of an inline call.
    fn inline_call_hint(&self) -> Option<&'static str> {
        let pos = self.text_pos()?;
        let (name, _) = self.call_around(&pos.prefix)?;
        Some(match name.as_str() {
            "strong" => "bold · * or Esc → end it · $ formula · _ italic",
            "emph" => "italic · _ or Esc → end it · $ formula · * bold",
            _ => "in the function's text · ] or Esc → leave it",
        })
    }

    /// The hint while the caret's paragraph is `#name` or `#name(args)` so
    /// far, with the caret at its end (space would make a block call).
    fn call_hint(&self) -> Option<&'static str> {
        let pos = self.text_pos()?;
        if pos.inlines.len() != 1
            || pos.offset != pos.len
            || !matches!(pos.block.kind, Block::Paragraph(_))
        {
            return None;
        }
        let (name, args) = input::call_head(&pos.chars())?;
        Some(match (self.call_shape(&name), args) {
            (Some(c), _) if !c.body => "this function takes no content",
            (_, Some(_)) => "space → block call with these arguments",
            (Some(c), None) if !c.title => {
                "space → block call · ( → its arguments, e.g. #align(center)"
            }
            (_, None) => "space → environment (e.g. #lemma, #proof) · ( → with a title",
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
    block.kind.inlines().is_some_and(|v| is_empty_text(v))
}

/// Inlines holding only an empty text.
fn is_empty_text(inlines: &[InlineNode]) -> bool {
    matches!(inlines, [InlineNode { kind: Inline::Text(s), .. }] if s.is_empty())
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
        Inline::Math(_) | Inline::Call { .. } => "",
    }
}
