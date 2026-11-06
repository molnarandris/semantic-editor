//! Editing commands: a document, its caret and its history. Every command
//! turns into [`Op`]s applied through [`History`], so everything is undoable.
//! Geometry (clicks, up/down, line start/end) is the widget's job; it hands
//! the resulting caret to [`Editor::set_caret`].
//!
//! Prose only for now: math rows are passed over as atoms by the arrow keys
//! and typing inside math is ignored. Raw Typst blocks are not editable here.

use crate::cursor::{CursorPath, Step, Target};
use crate::history::History;
use crate::ops::{Items, Op, Seq};
use crate::tree::{Block, BlockNode, Document, Inline, InlineNode};

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

#[derive(Debug)]
pub struct Editor {
    doc: Document,
    caret: CursorPath,
    history: History,
    last: Last,
}

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
        }
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn caret(&self) -> &CursorPath {
        &self.caret
    }

    /// Move the caret (e.g. after a click). Invalid carets are repaired.
    pub fn set_caret(&mut self, caret: CursorPath) {
        self.caret = self.doc.fix_cursor(&caret);
        self.last = Last::Other;
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Returns whether anything changed.
    pub fn undo(&mut self) -> bool {
        self.last = Last::Other;
        match self.history.undo(&mut self.doc) {
            Some(caret) => {
                self.caret = self.doc.fix_cursor(&caret);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.last = Last::Other;
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

    /// Insert typed text at the caret (text inlines only for now).
    ///
    /// Typst collapses runs of spaces and drops spaces at the start of a
    /// paragraph, so such spaces are not inserted at all: the model only
    /// holds text that is visible.
    pub fn insert_text(&mut self, s: &str) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let chars = pos.chars();
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

    /// Delete the char before the caret; at the start of a text, remove the
    /// formula before it or join with the previous block.
    pub fn backspace(&mut self) -> bool {
        let Some(pos) = self.text_pos() else {
            return self.backspace_block();
        };
        let block_id = pos.block.id;
        if pos.offset > 0 {
            let (op, after) = self.delete_char(&pos, pos.offset - 1);
            return self.commit(vec![op], after, Last::Deleting);
        }
        if pos.inline >= 2 {
            // Remove the formula before the caret; join the texts around it.
            let (ops, after) = remove_formula(block_id, pos.inlines, pos.inline - 1);
            return self.commit(ops, after, Last::Other);
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
        self.set_caret(caret);
        true
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

    /// Backspace at the start of a display-math or raw block: an empty
    /// paragraph before it is removed, otherwise the caret moves back.
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
        self.set_caret(caret);
        true
    }

    /// Delete the char after the caret; at the end of a text, remove the
    /// formula after it or join with the next block.
    pub fn delete(&mut self) -> bool {
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let block_id = pos.block.id;
        if pos.offset < pos.len {
            let (op, after) = self.delete_char(&pos, pos.offset);
            return self.commit(vec![op], after, Last::Deleting);
        }
        if pos.inline + 2 < pos.inlines.len() {
            let (ops, after) = remove_formula(block_id, pos.inlines, pos.inline + 1);
            return self.commit(ops, after, Last::Other);
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

    /// Split the paragraph at the caret. In a formula block or raw block,
    /// start a new paragraph after it.
    pub fn enter(&mut self) -> bool {
        let Some(pos) = self.text_pos() else {
            let Some(b) = self.doc.block_index(self.caret.block) else {
                return false;
            };
            let block = self.doc.paragraph(vec![]);
            let after = CursorPath {
                block: block.id,
                path: vec![Step::Index(0)],
                offset: 0,
            };
            let op = Op::Insert {
                seq: Seq::Blocks,
                index: b + 1,
                items: Items::Blocks(vec![block]),
            };
            return self.commit(vec![op], after, Last::Other);
        };
        let (b, i, c, len) = (pos.block_index, pos.inline, pos.offset, pos.len);
        let block_id = pos.block.id;
        let inlines = pos.inlines;
        let n = inlines.len();
        let Inline::Text(text) = &inlines[i].kind else {
            unreachable!("text_pos points at a text inline");
        };
        let tail: String = text.chars().skip(c).collect();
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
        let first = self.doc.text(&tail);
        let mut new_inlines = vec![first];
        new_inlines.extend(moved);
        let block = match heading {
            Some(level) => self.doc.heading(level, new_inlines),
            None => self.doc.paragraph(new_inlines),
        };
        let after = CursorPath {
            block: block.id,
            path: vec![Step::Index(0)],
            offset: 0,
        };
        ops.push(Op::Insert {
            seq: Seq::Blocks,
            index: b + 1,
            items: Items::Blocks(vec![block]),
        });
        self.commit(ops, after, Last::Other)
    }

    /// Move the caret one position left. Formulas count as one step.
    pub fn move_left(&mut self) -> bool {
        let next = self.left_of(&self.caret);
        self.move_to(next)
    }

    pub fn move_right(&mut self) -> bool {
        let next = self.right_of(&self.caret);
        self.move_to(next)
    }

    fn move_to(&mut self, next: Option<CursorPath>) -> bool {
        self.last = Last::Other;
        match next {
            Some(c) if c != self.caret => {
                self.caret = c;
                true
            }
            _ => false,
        }
    }

    fn left_of(&self, c: &CursorPath) -> Option<CursorPath> {
        let b = self.doc.block_index(c.block)?;
        if c.offset > 0 {
            // Raw blocks are one atom: jump to their start.
            let offset = match self.doc.resolve(c)? {
                Target::Raw { .. } => 0,
                _ => c.offset - 1,
            };
            return Some(CursorPath {
                offset,
                ..c.clone()
            });
        }
        if let (Some(_), [Step::Index(i)]) = (self.doc.blocks[b].kind.inlines(), &c.path[..])
            && *i >= 2
        {
            let path = vec![Step::Index(i - 2)];
            let len = self.doc.walk(&self.doc.blocks[b], &path).ok()?.len();
            return Some(CursorPath {
                block: c.block,
                path,
                offset: len,
            });
        }
        (b > 0).then(|| self.doc.block_end(b - 1))
    }

    fn right_of(&self, c: &CursorPath) -> Option<CursorPath> {
        let b = self.doc.block_index(c.block)?;
        let target = self.doc.resolve(c)?;
        let len = target.len();
        if c.offset < len {
            let offset = match target {
                Target::Raw { .. } => len,
                _ => c.offset + 1,
            };
            return Some(CursorPath {
                offset,
                ..c.clone()
            });
        }
        if let (Some(inlines), [Step::Index(i)]) = (self.doc.blocks[b].kind.inlines(), &c.path[..])
            && i + 2 < inlines.len()
        {
            return Some(CursorPath {
                block: c.block,
                path: vec![Step::Index(i + 2)],
                offset: 0,
            });
        }
        (b + 1 < self.doc.blocks.len()).then(|| self.doc.block_start(b + 1))
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
    fn arrows_pass_over_formulas_and_blocks() {
        let mut e = sample();
        caret_at(&mut e, 0, 0, 2);
        e.move_right(); // after the space
        assert_eq!(
            (e.caret().path.clone(), e.caret().offset),
            (vec![Step::Index(0)], 3)
        );
        e.move_right(); // over the formula
        assert_eq!(
            (e.caret().path.clone(), e.caret().offset),
            (vec![Step::Index(2)], 0)
        );
        e.move_left();
        assert_eq!(
            (e.caret().path.clone(), e.caret().offset),
            (vec![Step::Index(0)], 3)
        );
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
}
