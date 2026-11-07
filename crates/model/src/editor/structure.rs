//! Leaving formulas and blocks, splitting, headings, raw and diagram blocks.

use super::*;

impl Editor {
    /// Caret after the formula: in the text after an inline formula, or in
    /// the paragraph after a display formula (created if needed).
    pub(super) fn leave_formula(&mut self, p: &RowPos) -> bool {
        self.groups.clear();
        if p.base == 1 {
            let Step::Index(i) = p.path[0] else {
                return false;
            };
            return self.jump(cursor(p.block, vec![Step::Index(i + 1)], 0));
        }
        self.leave_block(p.block_index)
    }

    /// Caret to the start of the text block after block `b`, created if the
    /// next block is not a paragraph or heading.
    pub(super) fn leave_block(&mut self, b: usize) -> bool {
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

    pub(super) fn paragraph_after(&mut self, b: usize) -> bool {
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
        if self.source_block().is_some()
            && let Some(b) = self.doc.block_index(self.caret.block)
        {
            return self.leave_block(b);
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
    pub(super) fn retype(
        &mut self,
        b: usize,
        level: Option<u8>,
        strip: usize,
        after: CursorPath,
    ) -> bool {
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

    /// Insert a diagram or raw Typst block after the caret's block (in place
    /// of it, if that is an empty paragraph); the caret goes onto it.
    pub fn insert_block(&mut self, kind: NewBlock) -> bool {
        self.finish_word();
        self.selected = None;
        let Some(b) = self.doc.block_index(self.caret.block) else {
            return false;
        };
        let block = match kind {
            NewBlock::Diagram => self.doc.diagram(DIAGRAM_TEMPLATE),
            NewBlock::RawTypst => self.doc.raw(""),
        };
        let after = cursor(block.id, vec![], 0);
        let replace = is_empty_text_block(&self.doc.blocks[b]);
        let op = Op::Replace {
            seq: Seq::Blocks,
            index: if replace { b } else { b + 1 },
            count: usize::from(replace),
            items: Items::Blocks(vec![block]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// The source of a raw Typst or diagram block.
    pub fn block_source(&self, block: NodeId) -> Option<&str> {
        match &self.doc.block(block)?.kind {
            Block::RawTypst(s) | Block::Diagram(s) => Some(s),
            _ => None,
        }
    }

    /// Replace the source of a raw/diagram block (from the source editor dialog).
    /// Consecutive edits of one block are one undo step.
    pub fn set_block_source(&mut self, block: NodeId, source: &str) -> bool {
        let Some(old) = self.block_source(block) else {
            return false;
        };
        if old == source {
            return false;
        }
        let op = Op::Replace {
            seq: Seq::Text(block),
            index: 0,
            count: old.chars().count(),
            items: Items::Text(source.into()),
        };
        // The caret goes onto the block (its end), as when it was opened.
        let after = cursor(block, vec![], source.chars().count());
        self.commit(vec![op], after, Last::Source(block))
    }
}
