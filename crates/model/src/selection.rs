//! Selections: anchor + head cursors, normalized to whole subtrees in math.

use crate::cursor::{CursorPath, Step, Target};
use crate::tree::{Document, Inline, NodeId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: CursorPath,
    pub head: CursorPath,
}

/// A normalized, ordered selection range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelRange {
    /// Whole blocks `start..end` (by index).
    Blocks { start: usize, end: usize },
    /// Chars `start..end` of one text inline or raw block at `path`.
    Text {
        block: NodeId,
        path: Vec<Step>,
        start: usize,
        end: usize,
    },
    /// Nodes `start..end` of the math row at `path`.
    Row {
        block: NodeId,
        path: Vec<Step>,
        start: usize,
        end: usize,
    },
    /// Across inlines of a paragraph: from (inline, char) to (inline, char).
    /// Math inlines are always covered whole.
    Inlines {
        block: NodeId,
        start: (usize, usize),
        end: (usize, usize),
    },
}

impl Selection {
    pub fn caret(cursor: CursorPath) -> Self {
        Self {
            anchor: cursor.clone(),
            head: cursor,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Normalize to a range of whole subtrees. `None` if a cursor is invalid.
    pub fn normalized(&self, doc: &Document) -> Option<SelRange> {
        let (a, h) = (&self.anchor, &self.head);
        if !doc.is_valid(a) || !doc.is_valid(h) {
            return None;
        }
        if a.block != h.block {
            let (i, j) = (doc.block_index(a.block)?, doc.block_index(h.block)?);
            return Some(SelRange::Blocks {
                start: i.min(j),
                end: i.max(j) + 1,
            });
        }
        let block_id = a.block;
        let block = doc.block(block_id)?;
        let d = a
            .path
            .iter()
            .zip(&h.path)
            .take_while(|(x, y)| x == y)
            .count();

        if d == a.path.len() && d == h.path.len() {
            let (start, end) = (a.offset.min(h.offset), a.offset.max(h.offset));
            let path = a.path.clone();
            return Some(match doc.walk(block, &a.path).ok()? {
                Target::Row { .. } => SelRange::Row {
                    block: block_id,
                    path,
                    start,
                    end,
                },
                _ => SelRange::Text {
                    block: block_id,
                    path,
                    start,
                    end,
                },
            });
        }

        let prefix = &a.path[..d];
        if let Some(inlines) = block.kind.inlines().filter(|_| d == 0) {
            let bound = |c: &CursorPath| -> Option<((usize, usize), (usize, usize))> {
                let Some(Step::Index(i)) = c.path.first() else {
                    return None;
                };
                Some(match inlines.get(*i)?.kind {
                    Inline::Text(_) => ((*i, c.offset), (*i, c.offset)),
                    Inline::Math(_) => ((*i, 0), (*i + 1, 0)),
                })
            };
            let (sa, ea) = bound(a)?;
            let (sh, eh) = bound(h)?;
            return Some(SelRange::Inlines {
                block: block_id,
                start: sa.min(sh),
                end: ea.max(eh),
            });
        }

        if let Ok(Target::Row { .. }) = doc.walk(block, prefix) {
            // Diverging inside this row: cover each side's branch whole.
            let bound = |c: &CursorPath| match c.path.get(d) {
                Some(Step::Index(k)) => (*k, *k + 1),
                _ => (c.offset, c.offset),
            };
            let (sa, ea) = bound(a);
            let (sh, eh) = bound(h);
            return Some(SelRange::Row {
                block: block_id,
                path: prefix.to_vec(),
                start: sa.min(sh),
                end: ea.max(eh),
            });
        }

        // Diverging between children of one structure: select the structure.
        let Some(Step::Index(k)) = prefix.last() else {
            return None;
        };
        Some(SelRange::Row {
            block: block_id,
            path: prefix[..d - 1].to_vec(),
            start: *k,
            end: k + 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paragraph "a $x + (1)/(b)$ c".
    fn sample() -> Document {
        let mut doc = Document::empty();
        let a = doc.text("a ");
        let x = doc.ident("x");
        let plus = doc.sym("+");
        let one = doc.num("1");
        let b = doc.ident("b");
        let f = doc.frac(vec![one], vec![b]);
        let m = doc.math(vec![x, plus, f]);
        let c = doc.text(" c");
        let p = doc.paragraph(vec![a, m, c]);
        let p2 = doc.paragraph(vec![]);
        doc.blocks = vec![p, p2];
        doc
    }

    fn cur(doc: &Document, path: Vec<Step>, offset: usize) -> CursorPath {
        CursorPath {
            block: doc.blocks[0].id,
            path,
            offset,
        }
    }

    #[test]
    fn numerator_to_denominator_selects_fraction() {
        let doc = sample();
        let block = doc.blocks[0].id;
        let s = Selection {
            anchor: cur(&doc, vec![Step::Index(1), Step::Index(2), Step::Num], 0),
            head: cur(&doc, vec![Step::Index(1), Step::Index(2), Step::Den], 1),
        };
        assert_eq!(
            s.normalized(&doc),
            Some(SelRange::Row {
                block,
                path: vec![Step::Index(1)],
                start: 2,
                end: 3
            })
        );
    }

    #[test]
    fn row_position_to_inside_structure_covers_structure() {
        let doc = sample();
        let block = doc.blocks[0].id;
        let s = Selection {
            anchor: cur(&doc, vec![Step::Index(1)], 0),
            head: cur(&doc, vec![Step::Index(1), Step::Index(2), Step::Den], 0),
        };
        assert_eq!(
            s.normalized(&doc),
            Some(SelRange::Row {
                block,
                path: vec![Step::Index(1)],
                start: 0,
                end: 3
            })
        );
    }

    #[test]
    fn text_into_math_covers_math_whole() {
        let doc = sample();
        let block = doc.blocks[0].id;
        let s = Selection {
            anchor: cur(&doc, vec![Step::Index(1), Step::Index(2), Step::Num], 1),
            head: cur(&doc, vec![Step::Index(0)], 1),
        };
        assert_eq!(
            s.normalized(&doc),
            Some(SelRange::Inlines {
                block,
                start: (0, 1),
                end: (2, 0)
            })
        );
    }

    #[test]
    fn same_container_and_across_blocks() {
        let doc = sample();
        let block = doc.blocks[0].id;
        let s = Selection {
            anchor: cur(&doc, vec![Step::Index(2)], 2),
            head: cur(&doc, vec![Step::Index(2)], 0),
        };
        assert_eq!(
            s.normalized(&doc),
            Some(SelRange::Text {
                block,
                path: vec![Step::Index(2)],
                start: 0,
                end: 2
            })
        );
        let s = Selection {
            anchor: doc.block_start(1),
            head: cur(&doc, vec![Step::Index(0)], 0),
        };
        assert_eq!(
            s.normalized(&doc),
            Some(SelRange::Blocks { start: 0, end: 2 })
        );
    }
}
