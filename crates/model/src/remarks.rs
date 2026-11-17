//! Remarks: notes tied to a block or to a range inside one, kept beside the
//! text (not in the Typst source). A range's ends are (text or math row,
//! offset) pairs, so an edit elsewhere leaves them alone and an edit of the
//! same text or row moves them along ([`Document::apply`] calls
//! [`Document::move_remarks`] for every op, undo and redo too). If an end's
//! text or row goes, the remark falls back to the whole block; if the block
//! goes, the remark stays, detached (undo brings the block back).

use crate::cursor::{CursorPath, Target};
use crate::tree::{BlockNode, Document, NodeId};

#[derive(Clone, Debug, PartialEq)]
pub struct Remark {
    /// Unique within the document.
    pub id: u64,
    pub anchor: Anchor,
    pub text: String,
    pub resolved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// A whole block (a theorem, an equation, a paragraph).
    Block(NodeId),
    /// A range from a point in `block` to one in `end_block` (the same
    /// block, or a later one: across a display formula), in their texts or
    /// math rows.
    Range {
        block: NodeId,
        start: Point,
        end_block: NodeId,
        end: Point,
    },
}

impl Anchor {
    /// The block the remark is about.
    pub fn block(&self) -> NodeId {
        match *self {
            Anchor::Block(b) | Anchor::Range { block: b, .. } => b,
        }
    }
}

/// A position in a text (char offset) or a math row (node index), by the
/// text inline's (raw block's) or the row's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub node: NodeId,
    pub offset: usize,
}

/// What an op changed, for moving remarks: in the text or row `node` (if
/// the op edits one), `removed` items at `index` became `inserted` ones;
/// `drops` if nodes may have left the document.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edit {
    pub node: Option<NodeId>,
    pub index: usize,
    pub removed: usize,
    pub inserted: usize,
    pub drops: bool,
}

/// Where an end at `offset` goes. Text typed at a range's end stays out of
/// it (the start moves on, the end stays); an end in removed items goes to
/// the edge of what replaced them.
fn shift(offset: usize, e: &Edit, is_end: bool) -> usize {
    if offset < e.index || is_end && offset == e.index {
        offset
    } else if offset >= e.index + e.removed {
        offset + e.inserted - e.removed
    } else if is_end {
        e.index + e.inserted
    } else {
        e.index
    }
}

impl Document {
    /// Follow an applied op: shift the ends in the edited text or row; if
    /// nodes went, re-home ranges whose ends moved to another block and fall
    /// back to the block for ranges whose ends are gone.
    pub(crate) fn move_remarks(&mut self, e: &Edit) {
        if self.remarks.is_empty() {
            return;
        }
        for remark in &mut self.remarks {
            if let Anchor::Range { start, end, .. } = &mut remark.anchor {
                if Some(start.node) == e.node {
                    start.offset = shift(start.offset, e, false);
                }
                if Some(end.node) == e.node {
                    end.offset = shift(end.offset, e, true);
                }
            }
        }
        for i in 0..self.remarks.len() {
            let Anchor::Range {
                block,
                start,
                end_block,
                end,
            } = self.remarks[i].anchor
            else {
                continue;
            };
            let empty = start.node == end.node && start.offset >= end.offset;
            let at_home = |b: NodeId, p: Point| {
                self.block(b)
                    .and_then(|node| self.point_cursor(node, p))
                    .is_some_and(|c| c.block == b)
            };
            let anchor = if empty {
                Anchor::Block(block)
            } else if !e.drops || at_home(block, start) && at_home(end_block, end) {
                continue;
            } else if let (Some(home), Some(end_home)) = (self.home(start), self.home(end)) {
                Anchor::Range {
                    block: home,
                    start,
                    end_block: end_home,
                    end,
                }
            } else if self.block(block).is_some() {
                Anchor::Block(block)
            } else {
                // Detached: the block is gone (undo may bring it back).
                continue;
            };
            self.remarks[i].anchor = anchor;
        }
    }

    /// The block `point` is in now, if it is still in the document.
    fn home(&self, point: Point) -> Option<NodeId> {
        self.blocks
            .iter()
            .find_map(|b| self.point_cursor(b, point))
            .map(|c| c.block)
    }

    /// The text or row a caret position is in, as a [`Point`].
    pub fn point_at(&self, cursor: &CursorPath) -> Option<Point> {
        let node = match self.resolve(cursor)? {
            Target::Text { inline, .. } => inline.id,
            Target::Raw { block, .. } => block.id,
            Target::Row { row } => row.id,
            _ => return None,
        };
        Some(Point {
            node,
            offset: cursor.offset,
        })
    }

    /// The caret position of `point` inside `block` (or a block in it).
    pub fn point_cursor(&self, block: &BlockNode, point: Point) -> Option<CursorPath> {
        let c = self
            .positions(block)
            .into_iter()
            .find(|c| self.point_at(c).is_some_and(|p| p.node == point.node))?;
        Some(CursorPath {
            offset: point.offset,
            ..c
        })
    }

    /// Where caret position `c` is in document order: its top-level
    /// block's index and its index among that block's positions.
    pub fn doc_order(&self, c: &CursorPath) -> Option<(usize, usize)> {
        let top = self.top_index(c.block)?;
        let at = self
            .positions(&self.blocks[top])
            .iter()
            .position(|p| p == c)?;
        Some((top, at))
    }

    /// The caret positions a range remark spans, `None` for a whole block
    /// or a detached remark.
    pub fn remark_range(&self, remark: &Remark) -> Option<(CursorPath, CursorPath)> {
        let Anchor::Range {
            block,
            start,
            end_block,
            end,
        } = remark.anchor
        else {
            return None;
        };
        Some((
            self.point_cursor(self.block(block)?, start)?,
            self.point_cursor(self.block(end_block)?, end)?,
        ))
    }

    /// Add a remark; returns its id.
    pub fn add_remark(&mut self, anchor: Anchor, text: &str) -> u64 {
        let id = self.remarks.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        self.remarks.push(Remark {
            id,
            anchor,
            text: text.into(),
            resolved: false,
        });
        id
    }

    /// The remarks in the order of the text: by block, then by where the
    /// range starts (a whole block before ranges in it); detached last.
    pub fn remarks_in_order(&self) -> Vec<&Remark> {
        let key = |r: &Remark| {
            let Some(index) = self.top_index(r.anchor.block()) else {
                return (usize::MAX, 0);
            };
            let positions = self.positions(&self.blocks[index]);
            let at = match self.remark_range(r) {
                Some((start, _)) => positions.iter().position(|p| *p == start),
                None => {
                    let block = self.block(r.anchor.block());
                    positions
                        .iter()
                        .position(|p| block.is_some_and(|b| b.contains(p.block)))
                }
            };
            (index, at.unwrap_or(0))
        };
        let mut remarks: Vec<&Remark> = self.remarks.iter().collect();
        remarks.sort_by_cached_key(|r| key(r));
        remarks
    }
}
