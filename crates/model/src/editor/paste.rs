//! Paste: a fragment (read from the clipboard's Typst) put in at the caret.

use super::*;

impl Editor {
    /// Whether the caret is in a formula (pasted text is read as math).
    pub fn in_math(&self) -> bool {
        self.row_pos().is_some()
    }

    /// Put `fragment` in at the caret, replacing a selected range: math
    /// nodes into the formula, inlines into the text (joined with it at both
    /// ends), blocks between blocks (an empty paragraph is replaced, a
    /// paragraph split at the caret). Plain text is typed. One undo step.
    pub fn paste(&mut self, mut fragment: Fragment) -> bool {
        self.finish_word();
        let mut changed = false;
        if self.selection().is_some() {
            changed = self.delete_range();
        }
        self.anchor = None;
        self.selected = None;
        let last = if changed { Last::Chained } else { Last::Other };
        self.doc.adopt(&mut fragment);
        match fragment {
            Fragment::Plain(s) => self.insert_text(&s) || changed,
            Fragment::Math(nodes) => self.paste_math(nodes, last) || changed,
            Fragment::Inlines(inlines) => self.paste_inlines(inlines, last) || changed,
            Fragment::Blocks(blocks) => self.paste_blocks(blocks, last) || changed,
        }
    }

    fn paste_math(&mut self, nodes: Vec<MathNode>, last: Last) -> bool {
        if let Some(p) = self.row_pos() {
            let n = nodes.len();
            let op = Op::Insert {
                seq: Seq::Row(p.row),
                index: p.offset,
                items: Items::Math(nodes),
            };
            return self.commit(vec![op], p.at(p.offset + n), last);
        }
        // In text: as an inline formula.
        let (a, m, b) = (self.doc.text(""), self.doc.math(nodes), self.doc.text(""));
        self.paste_inlines(vec![a, m, b], last)
    }

    fn paste_inlines(&mut self, inlines: Vec<InlineNode>, last: Last) -> bool {
        let inlines = self.doc.normalized_inlines(inlines);
        if self.row_pos().is_some() {
            // One formula pasted into a formula: its nodes.
            if let [
                _,
                InlineNode {
                    kind: Inline::Math(row),
                    ..
                },
                _,
            ] = &inlines[..]
                && inlines
                    .iter()
                    .all(|i| !matches!(&i.kind, Inline::Text(t) if !t.is_empty()))
            {
                return self.paste_math(row.nodes.clone(), last);
            }
            let text: String = inlines.iter().map(text_of).collect();
            return self.insert_text(&text);
        }
        let Some(pos) = self.text_pos() else {
            let p = self.doc.paragraph(inlines);
            return self.paste_blocks(vec![p], last);
        };
        let (owner, id, index, offset) = (
            pos.owner,
            pos.inlines[pos.inline].id,
            pos.inline,
            pos.offset,
        );
        let prefix = pos.prefix.clone();
        let chars = pos.chars();
        let starts_block = index == 0 && prefix.is_empty();
        let head: String = chars[..offset].iter().collect();
        let tail: String = chars[offset..].iter().collect();
        let first = text_of(&inlines[0]).to_string();
        let first_part = joined_tail(&head, &first, starts_block);
        let mut ops = vec![Op::Delete {
            seq: Seq::Text(id),
            index: offset,
            count: chars.len() - offset,
        }];
        let after = if inlines.len() == 1 {
            let tail_part = joined_tail(&format!("{head}{first_part}"), &tail, starts_block);
            ops.push(Op::Insert {
                seq: Seq::Text(id),
                index: offset,
                items: Items::Text(format!("{first_part}{tail_part}")),
            });
            let at = offset + first_part.chars().count();
            cursor(
                pos.block.id,
                [&prefix[..], &[Step::Index(index)]].concat(),
                at,
            )
        } else {
            let mut moved = inlines[1..].to_vec();
            let n = moved.len();
            let last_text = moved.last_mut().expect("a text after the formula");
            let Inline::Text(t) = &mut last_text.kind else {
                return false;
            };
            let at = t.chars().count();
            let tail_part = joined_tail(t, &tail, false);
            t.push_str(&tail_part);
            ops.push(Op::Insert {
                seq: Seq::Text(id),
                index: offset,
                items: Items::Text(first_part),
            });
            ops.push(Op::Insert {
                seq: Seq::Inlines(owner),
                index: index + 1,
                items: Items::Inlines(moved),
            });
            cursor(
                pos.block.id,
                [&prefix[..], &[Step::Index(index + n)]].concat(),
                at,
            )
        };
        self.commit(ops, after, last)
    }

    fn paste_blocks(&mut self, blocks: Vec<BlockNode>, last: Last) -> bool {
        let Some(last_id) = blocks.last().map(|b| b.id) else {
            return false;
        };
        let (place, index, replace) = if let Some((place, offset)) = self.edge() {
            // Just before or after an environment.
            (place, place.index + offset, false)
        } else if let Some(pos) = self.text_pos() {
            let place = pos.place;
            let empty = pos.block.kind.inlines().is_some_and(|i| i.len() == 1)
                && pos.len == 0
                && !pos.nested();
            let at_start = !pos.nested() && pos.inline == 0 && pos.offset == 0;
            let at_end =
                !pos.nested() && pos.inline + 1 == pos.inlines.len() && pos.offset == pos.len;
            if empty {
                (place, place.index, true)
            } else if at_start {
                (place, place.index, false)
            } else if at_end {
                (place, place.index + 1, false)
            } else {
                // In the middle: split the paragraph, the blocks go between.
                if !self.enter() {
                    return false;
                }
                return self.insert_blocks(
                    place,
                    place.index + 1,
                    false,
                    blocks,
                    last_id,
                    Last::Chained,
                );
            }
        } else if let Some(place) = self.doc.place(self.caret.block) {
            // A raw block, a formula's edge: after the caret's block.
            (place, place.index + 1, false)
        } else {
            return false;
        };
        self.insert_blocks(place, index, replace, blocks, last_id, last)
    }

    /// Insert `blocks` at `index` of the list `place` is in (replacing the
    /// block at `place` if `replace`); the caret goes to the end of the
    /// last one.
    fn insert_blocks(
        &mut self,
        place: Place,
        index: usize,
        replace: bool,
        blocks: Vec<BlockNode>,
        last_id: NodeId,
        last: Last,
    ) -> bool {
        let mut ops = vec![Op::Insert {
            seq: place.seq(),
            index,
            items: Items::Blocks(blocks),
        }];
        if replace {
            ops.push(Op::Delete {
                seq: place.seq(),
                index: index + ops_len(&ops),
                count: 1,
            });
        }
        let before = self.caret.clone();
        if !self.commit(ops, before, last) {
            return false;
        }
        if let Some(b) = self.doc.block(last_id) {
            self.caret = self.doc.end_of(b);
        }
        true
    }
}

/// The number of blocks the first (insert) op puts in.
fn ops_len(ops: &[Op]) -> usize {
    match &ops[0] {
        Op::Insert { items, .. } => items.len(),
        _ => 0,
    }
}
