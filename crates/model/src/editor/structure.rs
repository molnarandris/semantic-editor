//! Leaving formulas and blocks, splitting, headings, raw and diagram blocks.

use super::*;

/// What splitting a text block gives: the ops, the inlines of the new
/// block and the caret's path in them.
type Split = (Vec<Op>, Vec<InlineNode>, Option<Vec<Step>>);

impl Editor {
    /// Caret after the formula: in the text after an inline formula, or in
    /// the paragraph after a display formula (created if needed).
    pub(super) fn leave_formula(&mut self, p: &RowPos) -> bool {
        self.groups.clear();
        if let Some((mut path, i)) = p.formula() {
            path.push(Step::Index(i + 1));
            return self.jump(cursor(p.block, path, 0));
        }
        self.leave_block(p.place)
    }

    /// Caret to the start of the text block after the block at `place`,
    /// created if the next block is not a paragraph or heading.
    pub(super) fn leave_block(&mut self, place: Place) -> bool {
        if let Some(next) = self.doc.sibling(place, 1)
            && next.kind.inlines().is_some()
        {
            let caret = self.doc.start_of(next);
            return self.jump(caret);
        }
        self.paragraph_after(place)
    }

    pub(super) fn paragraph_after(&mut self, place: Place) -> bool {
        let block = self.doc.paragraph(vec![]);
        let after = cursor(block.id, vec![Step::Index(0)], 0);
        let op = Op::Insert {
            seq: place.seq(),
            index: place.index + 1,
            items: Items::Blocks(vec![block]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// Insert an empty paragraph before the block at `place`; the caret
    /// stays, or with `enter` goes into it.
    pub(super) fn paragraph_before(&mut self, place: Place, enter: bool) -> bool {
        let block = self.doc.paragraph(vec![]);
        let after = if enter {
            self.doc.start_of(&block)
        } else {
            self.caret.clone()
        };
        let op = Op::Insert {
            seq: place.seq(),
            index: place.index,
            items: Items::Blocks(vec![block]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// The caret's place and offset when it is just before (0) or after (1)
    /// an environment.
    pub(super) fn edge(&self) -> Option<(Place, usize)> {
        match self.doc.resolve(&self.caret)? {
            Target::Edge { block } => Some((self.doc.place(block.id)?, self.caret.offset)),
            _ => None,
        }
    }

    /// Escape: drop the selection, else leave the innermost structure (caret
    /// just after it), else leave the formula, else the inline call, else
    /// the environment.
    pub fn escape(&mut self) -> bool {
        if self.anchor.take().is_some() {
            return true;
        }
        self.finish_word();
        if self.selected.take().is_some() {
            return true;
        }
        if self.source_block().is_some()
            && let Some(place) = self.doc.place(self.caret.block)
        {
            return self.leave_block(place);
        }
        if self.label_pos().is_some() {
            return self.leave_label();
        }
        if self.opts_pos().is_some() {
            return self.leave_opts();
        }
        let Some(p) = self.row_pos() else {
            if self.raw_pos().is_some() {
                return self.leave_raw();
            }
            if self.text_pos().is_some_and(|pos| pos.nested()) {
                return self.leave_call();
            }
            if let Some(env) = self.title_env() {
                return self.jump(cursor(env, vec![], 1));
            }
            // In the text of an environment, or at the edge of a nested one:
            // to just after the (enclosing) environment, outside it.
            let parent = match self.edge() {
                Some((place, _)) => place.parent,
                None => self.doc.place(self.caret.block).and_then(|p| p.parent),
            };
            let env = parent.and_then(|p| self.doc.env_around(p));
            return env.is_some_and(|env| self.jump(cursor(env, vec![], 1)));
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
        if self.selection().is_some() {
            self.delete_range();
        }
        self.anchor = None;
        self.selected = None;
        if self.label_pos().is_some() {
            return self.leave_label();
        }
        if self.opts_pos().is_some() {
            return self.leave_opts();
        }
        if let Some(pos) = self.raw_pos() {
            self.caret = pos.at(pos.inline + 1, 0);
        }
        if let Some(p) = self.row_pos() {
            // In a case of `cases`: a new case.
            if self.new_case(&p) {
                return true;
            }
            if let Some((mut path, i)) = p.formula() {
                path.push(Step::Index(i + 1));
                self.caret = cursor(p.block, path, 0);
            } else {
                return self.paragraph_after(p.place);
            }
        }
        // In a title or term: on to the body.
        if let Some(owner) = self.title_env().or_else(|| self.term_item()) {
            return self.body_start(owner).is_some_and(|c| self.jump(c));
        }
        // In a caption: out of the figure, just after it.
        if let Some(figure) = self.caption_of_caret() {
            return self.jump(cursor(figure, vec![], 1));
        }
        let Some(pos) = self.text_pos() else {
            let Some(place) = self.doc.place(self.caret.block) else {
                return false;
            };
            // At the start of a raw/diagram block or just before an
            // environment: a paragraph before it, as Enter at the start of a
            // line; the caret stays.
            if (self.source_block().is_some() || self.edge().is_some()) && self.caret.offset == 0 {
                return self.paragraph_before(place, false);
            }
            return self.paragraph_after(place);
        };
        let item = self.item_of(pos.block.id);
        if let Some(item) = item
            && is_empty_text_block(pos.block)
            && (pos.place.index > 0 || self.doc.siblings(Some(item)).is_some_and(|b| b.len() == 1))
        {
            let place = pos.place;
            return self.item_enter(item, place, None);
        }
        if let Some(env) = pos.place.parent
            && self.doc.is_env(env)
            && is_empty_text_block(pos.block)
            && self.doc.sibling(pos.place, 1).is_none()
            && pos.place.index > 0
        {
            // An empty last paragraph of an environment: it moves out, after
            // the environment (as Enter on an empty list item).
            let block = pos.block.clone();
            let after = self.caret.clone();
            let outer = self.doc.place(env).expect("environment of the caret");
            let ops = vec![
                delete_block(pos.place),
                Op::Insert {
                    seq: outer.seq(),
                    index: outer.index + 1,
                    items: Items::Blocks(vec![block]),
                },
            ];
            return self.commit(ops, after, Last::Other);
        }
        let place = pos.place;
        let old = pos.block.clone();
        let caret = self.caret.clone();
        let Some((mut ops, new_inlines, path)) = self.split_inlines(&old, &caret) else {
            return false;
        };
        let heading = match &old.kind {
            // Enter at the very end of a heading starts a paragraph.
            Block::Heading { level, .. } if path.is_some() => Some(*level),
            _ => None,
        };
        let block = match heading {
            Some(level) => self.doc.heading(level, new_inlines),
            None => self.doc.paragraph(new_inlines),
        };
        if let Some(item) = item {
            return self.item_enter(item, place, Some((ops, block)));
        }
        let path = path.unwrap_or(vec![Step::Index(0)]);
        let after = cursor(block.id, path, 0);
        ops.push(Op::Insert {
            seq: place.seq(),
            index: place.index + 1,
            items: Items::Blocks(vec![block]),
        });
        self.commit(ops, after, Last::Other)
    }

    /// Ops cutting the caret's text block at the caret, what follows it
    /// (the inlines of the new block) and the caret's path in them (`None`:
    /// nothing follows). Inline calls around the caret are split too: the
    /// new block starts with the rest of each, the caret inside (a call with
    /// nothing left after the caret is not continued).
    fn split_inlines(&mut self, block: &BlockNode, caret: &CursorPath) -> Option<Split> {
        let at = self.doc.inline_at(block, &caret.path)?;
        let (owner, i, list) = (at.owner, at.index, at.inlines.to_vec());
        let mut prefix = caret.path[..at.prefix].to_vec();
        let chars: Vec<char> = text_of(&list[i]).chars().collect();
        let (c, len) = (caret.offset, chars.len());
        let tail: String = chars[c..].iter().collect();
        let mut ops = vec![];
        if c < len {
            ops.push(Op::Delete {
                seq: Seq::Text(list[i].id),
                index: c,
                count: len - c,
            });
        }
        if i + 1 < list.len() {
            ops.push(Op::Delete {
                seq: Seq::Inlines(owner),
                index: i + 1,
                count: list.len() - i - 1,
            });
        }
        let first = match prefix.is_empty() {
            true => self.doc.text(tail.trim_start_matches(' ')),
            false => self.doc.text(&tail),
        };
        let mut rest = vec![first];
        rest.extend(list[i + 1..].iter().cloned());
        // Calls continued in the new block (they hold the caret there).
        let mut kept = 0;
        // Out through the calls around the caret, innermost first.
        while prefix.len() >= 2 {
            prefix.pop(); // Body
            let at = self.doc.inline_at(block, &prefix)?;
            let (owner, k, list) = (at.owner, at.index, at.inlines.to_vec());
            prefix.pop(); // Index(k)
            // The text after the call stays (emptied), as the list's end;
            // what it held moves on.
            let after = text_of(&list[k + 1]).to_string();
            if !after.is_empty() {
                ops.push(Op::Delete {
                    seq: Seq::Text(list[k + 1].id),
                    index: 0,
                    count: after.chars().count(),
                });
            }
            if k + 2 < list.len() {
                ops.push(Op::Delete {
                    seq: Seq::Inlines(owner),
                    index: k + 2,
                    count: list.len() - k - 2,
                });
            }
            let mut outer = vec![];
            if let Inline::Call { name, args, .. } = &list[k].kind
                && !is_empty_text(&rest)
            {
                outer.push(self.doc.inline_call(name, args, rest));
                kept += 1;
            }
            outer.push(self.doc.text(&after));
            outer.extend(list[k + 2..].iter().cloned());
            rest = outer;
        }
        let rest = self.doc.normalized_inlines(rest);
        let mut path = vec![];
        for _ in 0..kept {
            path.extend([Step::Index(1), Step::Body]);
        }
        path.push(Step::Index(0));
        let path = (!is_empty_text(&rest)).then_some(path);
        Some((ops, rest, path))
    }

    /// Make the caret's paragraph or heading a heading of `level`, or a
    /// paragraph with `None`. Returns false outside text blocks.
    pub fn set_heading(&mut self, level: Option<u8>) -> bool {
        self.finish_word();
        self.selected = None;
        let Some(place) = self.doc.place(self.caret.block) else {
            return false;
        };
        let current = match &self.doc.at(place).map(|b| &b.kind) {
            Some(Block::Heading { level, .. }) => Some(*level),
            Some(Block::Paragraph(_)) => None,
            _ => return false,
        };
        if current == level {
            return false;
        }
        let after = self.caret.clone();
        self.retype(place, level, 0, after)
    }

    /// Replace the text block at `place` by a heading/paragraph with the
    /// same inlines (and ids), minus the first `strip` chars.
    pub(super) fn retype(
        &mut self,
        place: Place,
        level: Option<u8>,
        strip: usize,
        after: CursorPath,
    ) -> bool {
        let Some(block) = self.doc.at(place) else {
            return false;
        };
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
            seq: place.seq(),
            index: place.index,
            count: 1,
            items: Items::Blocks(vec![BlockNode {
                id: block.id,
                label: match kind.labelable() {
                    true => block.label.clone(),
                    false => String::new(),
                },
                kind,
            }]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// Insert a diagram or raw Typst block after the caret's block (in place
    /// of it, if that is an empty paragraph); the caret goes onto it. A
    /// diagram with the caret in a formula is an inline one, a code atom at
    /// the caret (the caret goes after it).
    pub fn insert_block(&mut self, kind: NewBlock) -> bool {
        self.finish_word();
        self.selected = None;
        if kind == NewBlock::Diagram
            && let Some(p) = self.row_pos()
        {
            let node = self.doc.inline_diagram(INLINE_DIAGRAM_TEMPLATE);
            let op = row_insert(p.row, p.offset, node);
            return self.commit(vec![op], p.at(p.offset + 1), Last::Other);
        }
        if kind == NewBlock::Title {
            return self.insert_title();
        }
        let Some(mut place) = self.doc.place(self.caret.block) else {
            return false;
        };
        if kind == NewBlock::Bibliography {
            // At the top level, after the caret's top-level block.
            let Some(index) = self.doc.top_index(self.caret.block) else {
                return false;
            };
            place = Place {
                parent: None,
                index,
            };
        }
        let block = match kind {
            NewBlock::Diagram => self.doc.diagram(DIAGRAM_TEMPLATE),
            NewBlock::RawTypst => self.doc.raw(""),
            NewBlock::Bibliography => self.doc.bibliography_block(BIBLIOGRAPHY_TEMPLATE),
            NewBlock::Title => unreachable!("inserted above"),
        };
        let after = cursor(block.id, vec![], 0);
        let replace = self.doc.at(place).is_some_and(is_empty_text_block);
        let b = place.index;
        let op = Op::Replace {
            seq: place.seq(),
            index: if replace { b } else { b + 1 },
            count: usize::from(replace),
            items: Items::Blocks(vec![block]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// A title block at the start of the document, or the caret onto the one
    /// there is.
    fn insert_title(&mut self) -> bool {
        if let Some(b) = self
            .doc
            .blocks
            .iter()
            .find(|b| matches!(b.kind, Block::Title(_)))
        {
            let at = cursor(b.id, vec![], 0);
            self.set_caret(at);
            return true;
        }
        let block = self.doc.title_block(TITLE_TEMPLATE);
        let after = cursor(block.id, vec![], 0);
        let op = Op::Replace {
            seq: Place {
                parent: None,
                index: 0,
            }
            .seq(),
            index: 0,
            count: 0,
            items: Items::Blocks(vec![block]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// The source of a raw Typst or diagram block.
    pub fn block_source(&self, block: NodeId) -> Option<&str> {
        match self.doc.block(block).map(|b| &b.kind) {
            Some(
                Block::RawTypst(s) | Block::Diagram(s) | Block::Bibliography(s) | Block::Title(s),
            ) => Some(s),
            Some(_) => None,
            None => self.doc.code_source(block),
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
        // A code atom: replaced by one with the new source, same id; the
        // caret stays.
        if let Some((row, k, diagram)) = self
            .doc
            .find_node(block)
            .map(|(r, k)| (r.id, k, matches!(r.nodes[k].kind, MathKind::Diagram(_))))
        {
            let kind = match diagram {
                true => MathKind::Diagram(source.into()),
                false => MathKind::Code(source.into()),
            };
            let node = MathNode {
                id: block,
                kind,
                space: false,
            };
            let after = self.caret.clone();
            return self.commit(
                vec![self.row_replace(row, k, node)],
                after,
                Last::Source(block),
            );
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

    /// Replace the document header. Consecutive edits are one undo step;
    /// the caret stays where it is.
    pub fn set_header(&mut self, header: &str) -> bool {
        let old = &self.doc.header;
        if old == header {
            return false;
        }
        let op = Op::Replace {
            seq: Seq::Header,
            index: 0,
            count: old.chars().count(),
            items: Items::Text(header.into()),
        };
        let after = self.caret.clone();
        self.commit(vec![op], after, Last::Header)
    }
}
