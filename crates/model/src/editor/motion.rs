//! Caret moves that need no geometry: by position, by slot, ↑/↓ inside
//! structures (the widget handles line moves).

use super::*;

impl Editor {
    // ----- Moving -----

    /// Move the caret one position left, entering and leaving structures.
    pub fn move_left(&mut self) -> bool {
        self.finish_word();
        if let Some((offset, _)) = self.field_pos() {
            return offset > 0 && self.tab_to(offset - 1);
        }
        let next = self.neighbour(true);
        next.is_some_and(|c| self.jump(c))
    }

    pub fn move_right(&mut self) -> bool {
        self.finish_word();
        if let Some((offset, len)) = self.field_pos() {
            return offset < len && self.tab_to(offset + 1);
        }
        let next = self.neighbour(false);
        next.is_some_and(|c| self.jump(c))
    }

    /// The offset and length of the options tab or label the caret is in:
    /// fields beside the content, which ←/→ do not enter or leave.
    fn field_pos(&self) -> Option<(usize, usize)> {
        let (_, offset, len) = self.opts_pos().or_else(|| self.label_pos())?;
        Some((offset, len))
    }

    /// Home/End in an options tab or label: the start or end of its text.
    pub fn tab_edge(&self, end: bool) -> Option<CursorPath> {
        let (_, len) = self.field_pos()?;
        let mut c = self.caret.clone();
        c.offset = if end { len } else { 0 };
        Some(c)
    }

    /// ←/→ in an options tab or label: through its text, never out of it.
    fn tab_to(&mut self, offset: usize) -> bool {
        let mut c = self.caret.clone();
        c.offset = offset;
        self.jump(c)
    }

    pub(super) fn neighbour(&self, back: bool) -> Option<CursorPath> {
        // Positions of the top-level block, environments with their edges
        // (just before and after, outside them).
        let b = self.doc.top_index(self.caret.block)?;
        let list = self.doc.positions(&self.doc.blocks[b]);
        if let Some(i) = list.iter().position(|c| *c == self.caret) {
            let next = if back { i.checked_sub(1) } else { Some(i + 1) };
            if let Some(c) = next.and_then(|j| list.get(j)) {
                return Some(c.clone());
            }
        }
        if back {
            let before = self.doc.blocks.get(b.checked_sub(1)?)?;
            self.doc.positions(before).pop()
        } else {
            Some(self.doc.start_of(self.doc.blocks.get(b + 1)?))
        }
    }

    /// Tab / Shift+Tab: the next (previous) empty slot of the block; in the
    /// text of a list item, indent (outdent) the item.
    pub fn next_slot(&mut self, back: bool) -> bool {
        self.finish_word();
        if self.row_pos().is_none() {
            return self.indent_item(back);
        }
        let Some(block) = self.doc.block(self.caret.block) else {
            return false;
        };
        let list = self.doc.positions(block);
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
    /// scripts, case ↔ case of `cases`). The offset is 0; the widget picks
    /// the column. `None` means
    /// move by lines.
    pub fn vertical_target(&self, down: bool) -> Option<CursorPath> {
        // In a label: back to the end of its block.
        if self.label_pos().is_some() {
            return Some(self.label_exit());
        }
        // In an options tab: ↓ into the call, ↑ stays.
        if self.opts_pos().is_some() {
            if !down {
                return Some(self.caret.clone());
            }
            return Some(self.tab_exit());
        }
        // Beside a picture: ↑ into its options tab.
        if !down
            && self.caret.path.is_empty()
            && matches!(
                self.doc.block(self.caret.block).map(|b| &b.kind),
                Some(Block::Image { .. })
            )
        {
            return Some(cursor(self.caret.block, vec![Step::Opts], 0));
        }
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
            // The cases of `cases` (a call taking more arguments) are stacked.
            let stacked = matches!(&node.kind, MathKind::Call { name, .. }
                if self.math_call_shape(name).is_some_and(|s| s.variadic));
            let to = match (step, down) {
                (Step::Num, true) => Some(Step::Den),
                (Step::Den, false) => Some(Step::Num),
                (Step::Sup, true) | (Step::Sub, false) => Some(Step::Base),
                (Step::Base, true) => Some(Step::Sub),
                (Step::Base, false) => Some(Step::Sup),
                (Step::Arg(i), true) if stacked => Some(Step::Arg(i + 1)),
                (Step::Arg(i), false) if stacked && i > 0 => Some(Step::Arg(i - 1)),
                // Above a call's arguments: its options tab.
                (Step::Arg(_), false) if node.kind.tab().is_some() => Some(Step::Opts),
                _ => None,
            };
            if to == Some(Step::Opts) {
                return Some(cursor(p.block, child_path(parent, j, Step::Opts), 0));
            }
            if let Some(to) = to.filter(|t| node.child(*t).is_some()) {
                return Some(cursor(p.block, child_path(parent, j, to), 0));
            }
            path.truncate(n - 2);
        }
        None
    }
}
