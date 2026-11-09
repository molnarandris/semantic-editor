//! List items: making them, Enter, Backspace at their start, indenting.
//!
//! As in Typst there is no list node: consecutive items make a list, and an
//! item in an item's body is nested. Every edit here moves blocks with their
//! ids, so the caret stays where it is.

use super::*;

impl Editor {
    /// The list item whose body holds block `id` directly.
    pub(super) fn item_of(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.doc.place(id)?.parent?;
        self.doc.is_item(parent).then_some(parent)
    }

    /// The term item whose term the caret is in.
    pub(super) fn term_item(&self) -> Option<NodeId> {
        let block = self.doc.block(self.caret.block)?;
        (matches!(block.kind, Block::Item { .. }) && !self.caret.path.is_empty())
            .then_some(block.id)
    }

    /// The item the caret is in: the one holding its block, or whose term
    /// it is in.
    fn caret_item(&self) -> Option<NodeId> {
        self.term_item().or_else(|| self.item_of(self.caret.block))
    }

    /// `- `, `+ ` or `/ ` typed at the start of the paragraph at `place`:
    /// it becomes the body of a new item (a term item: caret in the term).
    pub(super) fn make_item(&mut self, place: Place, kind: ListKind) -> bool {
        let Some(mut para) = self.doc.at(place).cloned() else {
            return false;
        };
        if let Some(InlineNode {
            kind: Inline::Text(t),
            ..
        }) = para.kind.inlines_mut().and_then(|v| v.first_mut())
        {
            *t = t.chars().skip(1).collect::<String>().trim_start().into();
        }
        let para_id = para.id;
        let item = self.doc.item(kind, vec![para]);
        let after = match kind {
            ListKind::Term => cursor(item.id, vec![Step::Index(0)], 0),
            _ => cursor(para_id, vec![Step::Index(0)], 0),
        };
        let op = Op::Replace {
            seq: place.seq(),
            index: place.index,
            count: 1,
            items: Items::Blocks(vec![item]),
        };
        self.commit(vec![op], after, Last::Other)
    }

    /// An item like `item` (same kind, empty term) with this body.
    fn sibling_item(&mut self, item: NodeId, body: Vec<BlockNode>) -> Option<BlockNode> {
        let Block::Item { kind, .. } = self.doc.block(item)?.kind else {
            return None;
        };
        Some(self.doc.item(kind, body))
    }

    /// Enter in block `block` of item `item`, with `tail` the block split
    /// off after the caret (`None` if the block was empty): a new item after
    /// this one gets the tail and the blocks after it. An empty item moves
    /// out a level instead; an empty block after the first starts the new
    /// item itself.
    pub(super) fn item_enter(
        &mut self,
        item: NodeId,
        place: Place,
        split: Option<(Vec<Op>, BlockNode)>,
    ) -> bool {
        let (Some(ip), Some(body)) = (
            self.doc.place(item),
            self.doc.block(item).and_then(|b| b.kind.body()).cloned(),
        ) else {
            return false;
        };
        let k = place.index;
        // `from`: the first block of this item that moves to the new one.
        let (mut ops, moved, from, after) = match split {
            Some((ops, tail)) => {
                let after = self.doc.start_of(&tail);
                let mut moved = vec![tail];
                moved.extend(body[k + 1..].iter().cloned());
                (ops, moved, k + 1, after)
            }
            None if body.len() == 1 => return self.leave_item(item),
            None if k > 0 => (vec![], body[k..].to_vec(), k, self.caret.clone()),
            None => return false,
        };
        if body.len() > from {
            ops.push(Op::Delete {
                seq: Seq::Body(item),
                index: from,
                count: body.len() - from,
            });
        }
        let Some(new) = self.sibling_item(item, moved) else {
            return false;
        };
        let after = match &new.kind {
            Block::Item {
                kind: ListKind::Term,
                ..
            } => cursor(new.id, vec![Step::Index(0)], 0),
            _ => after,
        };
        ops.push(Op::Insert {
            seq: ip.seq(),
            index: ip.index + 1,
            items: Items::Blocks(vec![new]),
        });
        self.commit(ops, after, Last::Other)
    }

    /// Enter on an empty item: out one level (after the item it is nested
    /// in), or, at the outermost level, a plain paragraph again.
    fn leave_item(&mut self, item: NodeId) -> bool {
        if self.item_of(item).is_some() {
            return self.outdent(item);
        }
        self.unwrap_item(item, false)
    }

    /// Replace `item` by its body: appended to the previous item's body
    /// with `join` (Backspace: the text continues that item), else in place.
    pub(super) fn unwrap_item(&mut self, item: NodeId, join: bool) -> bool {
        let (Some(ip), Some(body)) = (
            self.doc.place(item),
            self.doc.block(item).and_then(|b| b.kind.body()).cloned(),
        ) else {
            return false;
        };
        // From the term: the start of the body.
        let after = if self.caret.block == item {
            self.doc.start_of(&body[0])
        } else {
            self.caret.clone()
        };

        let prev = self
            .doc
            .sibling(ip, -1)
            .filter(|b| join && matches!(b.kind, Block::Item { .. }))
            .and_then(|b| Some((b.id, b.kind.body()?.len())));
        let ops = match prev {
            Some((prev, len)) => vec![
                delete_block(ip),
                Op::Insert {
                    seq: Seq::Body(prev),
                    index: len,
                    items: Items::Blocks(body),
                },
            ],
            None => vec![Op::Replace {
                seq: ip.seq(),
                index: ip.index,
                count: 1,
                items: Items::Blocks(body),
            }],
        };
        self.commit(ops, after, Last::Other)
    }

    /// Tab in an item: nest it in the item before it (at the end of that
    /// item's body).
    fn indent(&mut self, item: NodeId) -> bool {
        let Some(ip) = self.doc.place(item) else {
            return false;
        };
        let Some((prev, len)) = self
            .doc
            .sibling(ip, -1)
            .filter(|b| matches!(b.kind, Block::Item { .. }))
            .and_then(|b| Some((b.id, b.kind.body()?.len())))
        else {
            return false;
        };
        let block = self.doc.at(ip).cloned().expect("the item");
        let ops = vec![
            delete_block(ip),
            Op::Insert {
                seq: Seq::Body(prev),
                index: len,
                items: Items::Blocks(vec![block]),
            },
        ];
        let after = self.caret.clone();
        self.commit(ops, after, Last::Other)
    }

    /// Shift+Tab (or Enter on an empty item) in a nested item: it moves out,
    /// after the item it was nested in, and takes that item's kind (a
    /// numbered item under a bullet becomes a bullet); the blocks after it
    /// there come along in its body, so the order stays. Term items keep
    /// their kind, and no item becomes one: they have a term the others lack.
    fn outdent(&mut self, item: NodeId) -> bool {
        let (Some(ip), Some(parent)) = (self.doc.place(item), self.item_of(item)) else {
            return false;
        };
        let (Some(pp), Some(siblings)) = (
            self.doc.place(parent),
            self.doc.block(parent).and_then(|b| b.kind.body()).cloned(),
        ) else {
            return false;
        };
        let mut moved = siblings[ip.index].clone();
        if let (
            Some(Block::Item { kind: outer, .. }),
            Block::Item { kind, .. },
        ) = (self.doc.block(parent).map(|b| &b.kind), &mut moved.kind)
        {
            if *kind != ListKind::Term && *outer != ListKind::Term {
                *kind = *outer;
            }
        }
        if let Some(body) = moved.kind.body_mut() {
            body.extend(siblings[ip.index + 1..].iter().cloned());
        }
        let mut ops = vec![Op::Delete {
            seq: Seq::Body(parent),
            index: ip.index,
            count: siblings.len() - ip.index,
        }];
        if ip.index == 0 {
            // The parent keeps a (now empty) body.
            let para = self.doc.paragraph(vec![]);
            ops.push(Op::Insert {
                seq: Seq::Body(parent),
                index: 0,
                items: Items::Blocks(vec![para]),
            });
        }
        ops.push(Op::Insert {
            seq: pp.seq(),
            index: pp.index + 1,
            items: Items::Blocks(vec![moved]),
        });
        let after = self.caret.clone();
        self.commit(ops, after, Last::Other)
    }

    /// Tab / Shift+Tab in the text of an item: indent or outdent it.
    pub(super) fn indent_item(&mut self, back: bool) -> bool {
        let Some(item) = self.caret_item() else {
            return false;
        };
        if back {
            self.outdent(item)
        } else {
            self.indent(item)
        }
    }
}
