//! Backspace and Delete: chars, nodes, slots, formulas, blocks, selections.

use super::*;

impl Editor {
    // ----- Deleting -----

    /// Remove the selection.
    pub(super) fn delete_selection(&mut self) -> bool {
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
                let Some(inlines) = self.doc.block(block).and_then(|b| b.kind.text_inlines())
                else {
                    return false;
                };
                let (ops, after) = remove_formula(block, inlines, inline);
                self.commit(ops, after, Last::Other)
            }
            Selected::Block { block } => {
                let Some(place) = self.doc.place(block) else {
                    return false;
                };
                let (ops, after) = self.remove_block(place);
                self.commit(ops, after, Last::Other)
            }
            Selected::Unwrap { env } => {
                let Some(place) = self.doc.place(env) else {
                    return false;
                };
                let Some(body) = self.doc.block(env).and_then(|b| b.kind.body()) else {
                    return false;
                };
                // The body takes the environment's place, ids and all, so
                // the caret stays where it is.
                let insert = Op::Insert {
                    seq: place.seq(),
                    index: place.index,
                    items: Items::Blocks(body.clone()),
                };
                let after = self.caret.clone();
                self.commit(vec![delete_block(place), insert], after, Last::Other)
            }
        }
    }

    /// Ops that remove the block at `place`, and the caret after: the end
    /// of the block before, or the start of the one after. The only block of
    /// the document or of an environment is replaced by an empty paragraph.
    pub(super) fn remove_block(&mut self, place: Place) -> (Vec<Op>, CursorPath) {
        if let Some(before) = self.doc.sibling(place, -1) {
            (vec![delete_block(place)], self.doc.end_of(before))
        } else if let Some(next) = self.doc.sibling(place, 1) {
            (vec![delete_block(place)], self.doc.start_of(next))
        } else {
            let para = self.doc.paragraph(vec![]);
            let after = cursor(para.id, vec![Step::Index(0)], 0);
            let insert = Op::Insert {
                seq: place.seq(),
                index: place.index + 1,
                items: Items::Blocks(vec![para]),
            };
            (vec![insert, delete_block(place)], after)
        }
    }

    /// The environment the caret is just before or after.
    fn edge_env(&self) -> Option<NodeId> {
        self.edge().map(|_| self.caret.block)
    }

    /// The caret's block, if it is a raw Typst or diagram block.
    pub(super) fn source_block(&self) -> Option<NodeId> {
        match self.doc.resolve(&self.caret)? {
            Target::Raw { block, .. } => Some(block.id),
            _ => None,
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
        if let Some(block) = self.source_block().or_else(|| self.edge_env()) {
            return self.select(Selected::Block { block });
        }
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let block_id = pos.block.id;
        if pos.offset > 0 {
            let (op, after) = self.delete_char(&pos, pos.offset - 1);
            return self.commit(vec![op], after, Last::Deleting);
        }
        if pos.inline == 0 && matches!(pos.block.kind, Block::Heading { .. }) {
            // A heading first becomes a paragraph; the next press joins.
            let (place, after) = (pos.place, self.caret.clone());
            return self.retype(place, None, 0, after);
        }
        if pos.inline >= 2 {
            return self.select(Selected::Formula {
                block: block_id,
                inline: pos.inline - 1,
            });
        }
        if let Block::Env { title, .. } = &pos.block.kind {
            // The start of a title: an empty one goes, caret into the body.
            if !is_empty_text(title) {
                return false;
            }
            let (Some(op), Some(after)) = (self.remove_title(block_id), self.body_start(block_id))
            else {
                return false;
            };
            return self.commit(vec![op], after, Last::Other);
        }
        if let Block::Item { term, .. } = &pos.block.kind {
            // The start of a term: an empty one drops the item's marker.
            return is_empty_text(term) && self.unwrap_item(block_id, true);
        }
        let place = pos.place;
        if place.index == 0
            && let Some(item) = self.item_of(block_id)
        {
            // The start of an item: after a term, into the term; else the
            // marker goes and the text continues the item before (if any).
            if let Some(Block::Item { term, .. }) = self.doc.block(item).map(|b| &b.kind)
                && let Some(last) = term.last()
            {
                let end = text_of(last).chars().count();
                return self.jump(cursor(item, vec![Step::Index(term.len() - 1)], end));
            }
            return self.unwrap_item(item, true);
        }
        let Some(before) = self.doc.sibling(place, -1) else {
            // The start of an environment: mark it (the next press unwraps
            // it), or remove it at once if it is still empty.
            let Some(env) = place.parent else {
                return false;
            };
            let empty = self
                .doc
                .block(env)
                .and_then(|b| b.kind.body())
                .is_some_and(|body| body.len() == 1 && is_empty_text_block(&body[0]));
            if empty {
                let env_place = self.doc.place(env).expect("environment of the caret");
                let (ops, after) = self.remove_block(env_place);
                return self.commit(ops, after, Last::Other);
            }
            return self.select(Selected::Unwrap { env });
        };
        if before.kind.inlines().is_some() {
            let (ops, after) = join_blocks(
                &self.doc,
                Place {
                    index: place.index - 1,
                    ..place
                },
            );
            return self.commit(ops, after, Last::Other);
        }
        // The previous block is math, raw Typst or an environment.
        let end = self.doc.end_of(before);
        if is_empty_text_block(pos.block) {
            return self.commit(vec![delete_block(place)], end, Last::Other);
        }
        self.jump(end)
    }

    pub(super) fn math_backspace(&mut self, p: RowPos) -> bool {
        let nodes = self.row_nodes(p.row).to_vec();
        if p.offset > 0 {
            let k = p.offset - 1;
            let node = &nodes[k];
            if node.is_structure() || node.source().is_some() {
                self.finish_word();
                return self.select_node(&p, k);
            }
            let in_word = self.word.as_ref() == Some(&self.caret);
            // Words, numbers and text lose their last char, primes one
            // prime; an empty text (being typed) goes.
            let shorter = match &node.kind {
                MathKind::Ident(s) | MathKind::Num(s) | MathKind::Text(s)
                    if s.chars().count() > 1
                        || matches!(node.kind, MathKind::Text(_)) && !s.is_empty() =>
                {
                    let mut short = s.clone();
                    short.pop();
                    Some(match node.kind {
                        MathKind::Ident(_) => MathKind::Ident(short),
                        MathKind::Num(_) => MathKind::Num(short),
                        _ => MathKind::Text(short),
                    })
                }
                MathKind::Primes(n) if *n > 1 => Some(MathKind::Primes(n - 1)),
                _ => None,
            };
            if let Some(kind) = shorter {
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
        let place = p.place;
        if p.base == 1 {
            let Step::Index(i) = p.path[0] else {
                return false;
            };
            if p.len > 0 {
                return self.move_left();
            }
            let inlines = self
                .doc
                .at(place)
                .and_then(|b| b.kind.text_inlines())
                .expect("text block");
            let (ops, after) = remove_formula(p.block, inlines, i);
            return self.commit(ops, after, Last::Other);
        }
        // Display math.
        if p.len == 0 {
            let (ops, after) = self.remove_block(place);
            return self.commit(ops, after, Last::Other);
        }
        if self.doc.sibling(place, -1).is_some_and(is_empty_text_block) {
            let after = self.caret.clone();
            let before = Place {
                index: place.index - 1,
                ..place
            };
            return self.commit(vec![delete_block(before)], after, Last::Other);
        }
        self.move_left()
    }

    /// Backspace in an empty slot: remove it, unwrapping the structure when
    /// only one meaningful part remains.
    pub(super) fn remove_slot(&mut self, p: &RowPos) -> bool {
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
            // All arguments empty: the call goes. An extra argument of a
            // call that takes more (`cases`) goes, caret to the end of the
            // one before.
            MathKind::Call { name, args } => {
                let Step::Arg(i) = step else {
                    return false;
                };
                let min = self.math_call_shape(name).map_or(usize::MAX, |s| s.slots);
                if args.iter().all(|a| a.nodes.is_empty()) {
                    (vec![remove], at(j))
                } else if i > 0 && args.len() > min {
                    let mut n = node.clone();
                    if let MathKind::Call { args, .. } = &mut n.kind {
                        args.remove(i);
                    }
                    let prev = cursor(
                        p.block,
                        child_path(&parent, j, Step::Arg(i - 1)),
                        args[i - 1].nodes.len(),
                    );
                    (vec![row_replace(prow, j, n)], prev)
                } else {
                    return self.move_left();
                }
            }
            _ => return false,
        };
        self.commit(ops, after, Last::Other)
    }

    /// Delete char `k` of the caret's text; also a space that would end up
    /// doubled or leading the paragraph. Returns the op and the new caret.
    pub(super) fn delete_char(&self, pos: &TextPos, k: usize) -> (Op, CursorPath) {
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

    /// Delete the char or node after the caret (structures and formulas:
    /// select first); at the end of a text, join with the next block.
    pub fn delete(&mut self) -> bool {
        if self.selected.is_some() {
            return self.delete_selection();
        }
        if let Some(block) = self.source_block().or_else(|| self.edge_env()) {
            return self.select(Selected::Block { block });
        }
        if let Some(p) = self.row_pos() {
            self.finish_word();
            let Some(node) = self.row_nodes(p.row).get(p.offset) else {
                return false;
            };
            if node.is_structure() || node.source().is_some() {
                return self.select_node(&p, p.offset);
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
            return self.select(Selected::Formula {
                block: block_id,
                inline: pos.inline + 1,
            });
        }
        if pos.block.kind.body().is_some() {
            return false; // the end of a title or term
        }
        let place = pos.place;
        let Some(next) = self.doc.sibling(place, 1) else {
            return false;
        };
        if next.kind.inlines().is_some() {
            let (ops, after) = join_blocks(&self.doc, place);
            return self.commit(ops, after, Last::Other);
        }
        if is_empty_text_block(pos.block) {
            let after = self.doc.start_of(next);
            return self.commit(vec![delete_block(place)], after, Last::Other);
        }
        false
    }
}

/// The text to append to `head` when joining it with `tail`, without
/// doubling a space at the seam or leading the paragraph with one.
pub(super) fn joined_tail(head: &str, tail: &str, head_starts_block: bool) -> String {
    if head.ends_with(char::is_whitespace) || head.is_empty() && head_starts_block {
        tail.trim_start().into()
    } else {
        tail.into()
    }
}

/// Remove formula `m` of a paragraph and join the texts on either side.
pub(super) fn remove_formula(
    block: crate::NodeId,
    inlines: &[InlineNode],
    m: usize,
) -> (Vec<Op>, CursorPath) {
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

/// Append the text block after `first` (in the same list) to the text
/// block at `first`.
pub(super) fn join_blocks(doc: &Document, first: Place) -> (Vec<Op>, CursorPath) {
    let a = doc.at(first).expect("first block");
    let b = doc.sibling(first, 1).expect("second block");
    let second = Place {
        index: first.index + 1,
        ..first
    };
    let (ai, bi) = (
        a.kind.inlines().expect("text block"),
        b.kind.inlines().expect("text block"),
    );
    let last = ai.last().expect("paragraph invariant");
    let len = text_of(last).chars().count();
    let mut ops = vec![
        // Remove the second block first so its nodes never exist twice.
        delete_block(second),
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
