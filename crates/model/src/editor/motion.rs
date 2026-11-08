//! Caret moves that need no geometry: by position, by slot, ↑/↓ inside
//! structures (the widget handles line moves).

use super::*;

impl Editor {
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
            Some(self.doc.end_of(self.doc.blocks.get(b.checked_sub(1)?)?))
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
}
