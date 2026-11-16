//! Backspace and Delete: chars, nodes, slots, formulas, blocks, selections.

use super::*;

impl Editor {
    // ----- Deleting -----

    /// Delete the selected range (Backspace, Delete, typing over it); the
    /// caret goes to where it started. A math range becomes the node
    /// selection first, so `/`, `^`, `_` typed over it still wrap it.
    pub(super) fn delete_range(&mut self) -> bool {
        let range = self.selection();
        self.anchor = None;
        let Some(range) = range else {
            return false;
        };
        match range {
            SelRange::Row {
                block,
                path,
                start,
                end,
            } => {
                self.selected = Some(Selected::Nodes {
                    block,
                    path,
                    start,
                    end,
                });
                self.delete_selection()
            }
            SelRange::Text {
                block,
                path,
                start,
                end,
            } => {
                let Some(b) = self.doc.block(block) else {
                    return false;
                };
                let seq = match self.doc.walk(b, &path) {
                    Ok(Target::Text { inline, .. }) => Seq::Text(inline.id),
                    Ok(Target::Label { block, .. }) => Seq::Label(block.id),
                    Ok(Target::Opts { id, .. }) => Seq::Opts(id),
                    Ok(Target::Code { id, .. }) => Seq::Text(id),
                    // Raw Typst (its start to its end): the block goes.
                    Ok(Target::Raw { .. }) => {
                        let Some(place) = self.doc.place(block) else {
                            return false;
                        };
                        let (ops, after) = self.remove_block(place);
                        return self.commit(ops, after, Last::Other);
                    }
                    _ => return false,
                };
                let op = Op::Delete {
                    seq,
                    index: start,
                    count: end - start,
                };
                self.commit(vec![op], cursor(block, path, start), Last::Other)
            }
            SelRange::Inlines {
                block,
                owner,
                prefix,
                start,
                end,
            } => self.delete_inlines(block, owner, &prefix, start, end),
            SelRange::Blocks { start, end } => self.delete_blocks(start, end),
        }
    }

    /// Delete from (inline, char) `start` to `end` of the inline list owned
    /// by `owner` at `prefix`: the texts at both ends are joined.
    fn delete_inlines(
        &mut self,
        block: NodeId,
        owner: NodeId,
        prefix: &[Step],
        start: (usize, usize),
        end: (usize, usize),
    ) -> bool {
        let Some(b) = self.doc.block(block) else {
            return false;
        };
        let first_path = [prefix, &[Step::Index(0)]].concat();
        let Some(at) = self.doc.inline_at(b, &first_path) else {
            return false;
        };
        let list = at.inlines;
        let (mut i, mut c) = start;
        let (j, c2) = end;
        // A range starting with a formula or call starts at the end of the
        // text before it.
        if !matches!(list.get(i).map(|n| &n.kind), Some(Inline::Text(_))) {
            let Some(k) = i.checked_sub(1) else {
                return false;
            };
            i = k;
            c = text_of(&list[i]).chars().count();
        }
        let (Some(first), Some(last)) = (list.get(i), list.get(j)) else {
            return false;
        };
        let (ft, lt) = (text_of(first), text_of(last));
        let ops = if i == j {
            vec![Op::Delete {
                seq: Seq::Text(first.id),
                index: c,
                count: c2.saturating_sub(c),
            }]
        } else {
            let head: String = ft.chars().take(c).collect();
            let tail: String = lt.chars().skip(c2).collect();
            let starts_block = i == 0 && prefix.is_empty();
            vec![
                Op::Delete {
                    seq: Seq::Inlines(owner),
                    index: i + 1,
                    count: j - i,
                },
                Op::Delete {
                    seq: Seq::Text(first.id),
                    index: c,
                    count: ft.chars().count() - c,
                },
                Op::Insert {
                    seq: Seq::Text(first.id),
                    index: c,
                    items: Items::Text(joined_tail(&head, &tail, starts_block)),
                },
            ]
        };
        let after = cursor(block, [prefix, &[Step::Index(i)]].concat(), c);
        self.commit(ops, after, Last::Other)
    }

    /// Delete top-level blocks `start..end`; an emptied document gets an
    /// empty paragraph. The caret goes to the start of the block after them,
    /// else to the end of the block before.
    fn delete_blocks(&mut self, start: usize, end: usize) -> bool {
        let n = self.doc.blocks.len();
        let mut ops = vec![Op::Delete {
            seq: Seq::Blocks,
            index: start,
            count: end - start,
        }];
        let after = if end < n {
            self.doc.start_of(&self.doc.blocks[end])
        } else if start > 0 {
            self.doc.end_of(&self.doc.blocks[start - 1])
        } else {
            let para = self.doc.paragraph(vec![]);
            let after = cursor(para.id, vec![Step::Index(0)], 0);
            ops.push(Op::Insert {
                seq: Seq::Blocks,
                index: 0,
                items: Items::Blocks(vec![para]),
            });
            after
        };
        self.commit(ops, after, Last::Other)
    }

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
            Selected::Inline { block, path } => {
                let Some((ops, after)) = self.remove_inline(block, &path) else {
                    return false;
                };
                self.commit(ops, after, Last::Other)
            }
            Selected::Block { block } => {
                let Some(place) = self.doc.place(block) else {
                    return false;
                };
                let (ops, after) = self.remove_block(place);
                self.commit(ops, after, Last::Other)
            }
            Selected::UnwrapCall { block, path } => {
                let Some((ops, after)) = self.unwrap_inline(block, &path) else {
                    return false;
                };
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
                // the caret stays where it is; a figure's caption becomes a
                // paragraph.
                let body = body
                    .iter()
                    .cloned()
                    .map(|mut b| {
                        if let Block::Caption(inlines) = b.kind {
                            b.kind = Block::Paragraph(inlines);
                        }
                        b
                    })
                    .collect();
                let insert = Op::Insert {
                    seq: place.seq(),
                    index: place.index,
                    items: Items::Blocks(body),
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
        // A space typed but not placed yet (held): forget it.
        if self.held_space.take().as_ref() == Some(&self.caret) {
            return true;
        }
        if self.selection().is_some() {
            return self.delete_range();
        }
        if self.selected.is_some() {
            return self.delete_selection();
        }
        if let Some(p) = self.row_pos() {
            return self.math_backspace(p);
        }
        if let Some(block) = self.source_block().or_else(|| self.edge_env()) {
            return self.select(Selected::Block { block });
        }
        if let Some((block, offset, _)) = self.label_pos() {
            if offset == 0 {
                return self.move_left();
            }
            let op = Op::Delete {
                seq: Seq::Label(block),
                index: offset - 1,
                count: 1,
            };
            let after = cursor(block, vec![Step::Label], offset - 1);
            return self.commit(vec![op], after, Last::Deleting);
        }
        if let Some((node, offset, _)) = self.opts_pos() {
            if offset == 0 {
                return self.move_left();
            }
            let op = Op::Delete {
                seq: Seq::Opts(node),
                index: offset - 1,
                count: 1,
            };
            let mut after = self.caret.clone();
            after.offset = offset - 1;
            return self.commit(vec![op], after, Last::Deleting);
        }
        // Code in a formula, as inline code: chars go; at its start, empty
        // code goes, else ←.
        if let Some((node, offset, len)) = self.math_code_pos() {
            if offset == 0 {
                return match len {
                    0 => self.leave_math_code(),
                    _ => self.move_left(),
                };
            }
            let op = Op::Delete {
                seq: Seq::Text(node),
                index: offset - 1,
                count: 1,
            };
            let mut after = self.caret.clone();
            after.offset = offset - 1;
            return self.commit(vec![op], after, Last::Deleting);
        }
        if let Some(pos) = self.raw_pos() {
            // Inline code: chars go; at its start, empty code goes, else ←.
            if pos.offset > 0 {
                let op = Op::Delete {
                    seq: Seq::Text(pos.inlines[pos.inline].id),
                    index: pos.offset - 1,
                    count: 1,
                };
                let after = pos.at(pos.inline, pos.offset - 1);
                return self.commit(vec![op], after, Last::Deleting);
            }
            if pos.len == 0 {
                return self.leave_raw();
            }
            return self.move_left();
        }
        let Some(pos) = self.text_pos() else {
            return false;
        };
        let block_id = pos.block.id;
        if pos.offset > 0 {
            let (op, after) = self.delete_char(&pos, pos.offset - 1);
            return self.commit(vec![op], after, Last::Deleting);
        }
        if let Some((path, label)) = self.ref_before() {
            // The reference being typed loses its last char; an empty one goes.
            if label.is_empty() {
                self.word = None;
                let Some((ops, after)) = self.remove_inline(block_id, &path) else {
                    return false;
                };
                return self.commit(ops, after, Last::Deleting);
            }
            let mut short = label;
            short.pop();
            let ok = self.relabel(&path, &short, Last::Deleting);
            self.word = Some(self.caret.clone());
            return ok;
        }
        if pos.inline >= 2 {
            return self.select(Selected::Inline {
                block: block_id,
                path: pos.at(pos.inline - 1, 0).path,
            });
        }
        if pos.nested() {
            // The start of a call's body: an empty call goes, else it is
            // marked, and the next Backspace unwraps it.
            let call = pos.prefix[..pos.prefix.len() - 1].to_vec();
            if pos.inlines.len() == 1 && pos.len == 0 {
                let Some((ops, after)) = self.remove_inline(block_id, &call) else {
                    return false;
                };
                return self.commit(ops, after, Last::Other);
            }
            return self.select(Selected::UnwrapCall {
                block: block_id,
                path: call,
            });
        }
        if pos.inline == 0 && matches!(pos.block.kind, Block::Heading { .. }) {
            // A heading first becomes a paragraph; the next press joins.
            let (place, after) = (pos.place, self.caret.clone());
            return self.retype(place, None, 0, after);
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
        if let Block::Caption(inlines) = &pos.block.kind {
            // The start of a caption: an empty one goes, else ←.
            if !is_empty_text(inlines) {
                return self.move_left();
            }
            let place = pos.place;
            let Some(before) = self.doc.sibling(place, -1) else {
                return false;
            };
            let after = self.doc.end_of(before);
            return self.commit(vec![delete_block(place)], after, Last::Other);
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
            // Empty: one empty paragraph (a figure's: and an empty caption).
            let empty =
                self.doc.block(env).and_then(|b| b.kind.body()).is_some_and(
                    |body| match &body[..] {
                        [only] => is_empty_text_block(only),
                        [
                            para,
                            BlockNode {
                                kind: Block::Caption(caption),
                                ..
                            },
                        ] => is_empty_text_block(para) && is_empty_text(caption),
                        _ => false,
                    },
                );
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

    /// Backspace or Delete in a gap holding a space that shows (next to
    /// a bar or quoted text): the space goes first.
    fn delete_gap_space(&mut self, p: &RowPos) -> Option<bool> {
        if !self.gap_space(p.row, p.offset) {
            return None;
        }
        self.finish_word();
        let op = self.set_gap_space(p.row, p.offset, false)?;
        let after = self.caret.clone();
        Some(self.commit(vec![op], after, Last::Deleting))
    }

    pub(super) fn math_backspace(&mut self, p: RowPos) -> bool {
        if let Some(done) = self.delete_gap_space(&p) {
            return done;
        }
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
                let op = self.row_replace(
                    p.row,
                    k,
                    MathNode {
                        id: node.id,
                        kind,
                        space: false,
                    },
                );
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
        if p.base > 0 {
            if p.len > 0 {
                return self.move_left();
            }
            let Some((ops, after)) = self.remove_inline(p.block, &p.path[..p.base]) else {
                return false;
            };
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
                        (vec![self.row_replace(prow, j, n)], base_end)
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
            MathKind::Call { name, args, .. } => {
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
                    (vec![self.row_replace(prow, j, n)], prev)
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
        let ws = |i: usize| chars.get(i).is_some_and(|&c| collapses(c));
        let space = |i: usize| chars.get(i) == Some(&' ');
        // A space that would lead the paragraph or a line goes too.
        let leading = (k == 0 && pos.inline == 0 && !pos.nested() || k > 0 && chars[k - 1] == '\n')
            && space(k + 1);
        let doubled = k > 0 && ws(k - 1) && ws(k + 1);
        let (index, count) = if leading {
            (k, 2)
        } else if doubled && space(k - 1) {
            (k - 1, 2)
        } else if doubled && space(k + 1) {
            (k, 2)
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
        if self.selection().is_some() {
            return self.delete_range();
        }
        if self.selected.is_some() {
            return self.delete_selection();
        }
        if let Some(block) = self.source_block().or_else(|| self.edge_env()) {
            return self.select(Selected::Block { block });
        }
        if let Some((block, offset, len)) = self.label_pos() {
            if offset == len {
                return self.move_right();
            }
            let op = Op::Delete {
                seq: Seq::Label(block),
                index: offset,
                count: 1,
            };
            let after = self.caret.clone();
            return self.commit(vec![op], after, Last::Deleting);
        }
        if let Some((node, offset, len)) = self.opts_pos() {
            if offset == len {
                return self.move_right();
            }
            let op = Op::Delete {
                seq: Seq::Opts(node),
                index: offset,
                count: 1,
            };
            let after = self.caret.clone();
            return self.commit(vec![op], after, Last::Deleting);
        }
        if let Some((node, offset, len)) = self.math_code_pos() {
            if offset == len {
                return self.move_right();
            }
            let op = Op::Delete {
                seq: Seq::Text(node),
                index: offset,
                count: 1,
            };
            let after = self.caret.clone();
            return self.commit(vec![op], after, Last::Deleting);
        }
        if let Some(pos) = self.raw_pos() {
            if pos.offset == pos.len {
                return self.move_right();
            }
            let op = Op::Delete {
                seq: Seq::Text(pos.inlines[pos.inline].id),
                index: pos.offset,
                count: 1,
            };
            let after = self.caret.clone();
            return self.commit(vec![op], after, Last::Deleting);
        }
        if let Some(p) = self.row_pos() {
            self.finish_word();
            if let Some(done) = self.delete_gap_space(&p) {
                return done;
            }
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
            return self.select(Selected::Inline {
                block: block_id,
                path: pos.at(pos.inline + 1, 0).path,
            });
        }
        if pos.nested() || pos.block.kind.body().is_some() {
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
    if head.ends_with(collapses) || head.is_empty() && head_starts_block {
        tail.trim_start_matches(' ').into()
    } else {
        tail.into()
    }
}

/// Remove inline `m` (a formula or call) of the list owned by `owner` at
/// `prefix` in `block`, and join the texts on either side.
pub(super) fn remove_formula(
    block: crate::NodeId,
    owner: crate::NodeId,
    prefix: &[Step],
    inlines: &[InlineNode],
    m: usize,
) -> (Vec<Op>, CursorPath) {
    let before = &inlines[m - 1];
    let after = &inlines[m + 1];
    let len = text_of(before).chars().count();
    let starts_block = m == 1 && prefix.is_empty();
    let ops = vec![
        Op::Delete {
            seq: Seq::Inlines(owner),
            index: m,
            count: 2,
        },
        Op::Insert {
            seq: Seq::Text(before.id),
            index: len,
            items: Items::Text(joined_tail(text_of(before), text_of(after), starts_block)),
        },
    ];
    let mut path = prefix.to_vec();
    path.push(Step::Index(m - 1));
    let caret = CursorPath {
        block,
        path,
        offset: len,
    };
    (ops, caret)
}

impl Editor {
    /// Ops replacing the inline call that `path` leads to by its body (the
    /// text around it joined with the body's first and last text), and the
    /// caret after: where the body started.
    fn unwrap_inline(&self, block: NodeId, path: &[Step]) -> Option<(Vec<Op>, CursorPath)> {
        let b = self.doc.block(block)?;
        let at = self.doc.inline_at(b, path)?;
        if at.prefix + 1 != path.len() || at.index == 0 {
            return None;
        }
        let m = at.index;
        let body = at.inlines.get(m)?.body()?;
        let (before, after) = (&at.inlines[m - 1], at.inlines.get(m + 1)?);
        let len = text_of(before).chars().count();
        let starts_block = m == 1 && at.prefix == 0;
        let first = text_of(body.first()?);
        let mut ops = vec![Op::Delete {
            seq: Seq::Inlines(at.owner),
            index: m,
            count: 2,
        }];
        if body.len() == 1 {
            let joined = format!("{first}{}", text_of(after));
            ops.push(Op::Insert {
                seq: Seq::Text(before.id),
                index: len,
                items: Items::Text(joined_tail(text_of(before), &joined, starts_block)),
            });
        } else {
            let mut moved = body[1..].to_vec();
            let last = moved.last_mut()?;
            if let Inline::Text(t) = &mut last.kind {
                let tail = joined_tail(t, text_of(after), false);
                t.push_str(&tail);
            }
            ops.push(Op::Insert {
                seq: Seq::Inlines(at.owner),
                index: m,
                items: Items::Inlines(moved),
            });
            ops.push(Op::Insert {
                seq: Seq::Text(before.id),
                index: len,
                items: Items::Text(joined_tail(text_of(before), first, starts_block)),
            });
        }
        let mut caret_path = path[..at.prefix].to_vec();
        caret_path.push(Step::Index(m - 1));
        let caret = CursorPath {
            block,
            path: caret_path,
            offset: len,
        };
        Some((ops, caret))
    }

    /// Ops removing the inline formula or call that `path` leads to, and the
    /// caret after.
    pub(super) fn remove_inline(
        &self,
        block: NodeId,
        path: &[Step],
    ) -> Option<(Vec<Op>, CursorPath)> {
        let b = self.doc.block(block)?;
        let at = self.doc.inline_at(b, path)?;
        if at.prefix + 1 != path.len() || at.index == 0 {
            return None;
        }
        Some(remove_formula(
            block,
            at.owner,
            &path[..at.prefix],
            at.inlines,
            at.index,
        ))
    }
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
