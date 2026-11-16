//! All mutations of a [`Document`] go through [`Op`] values applied by
//! [`Document::apply`], which returns the inverse op.
//!
//! Ops address containers by stable [`NodeId`], so an op stays meaningful
//! no matter what happens elsewhere in the tree. Moved nodes keep their ids,
//! which makes every inverse exact.

use std::fmt;

use crate::cursor::Step;
use crate::remarks::Edit;
use crate::tree::{
    Block, BlockNode, Document, Inline, InlineNode, MathKind, MathNode, NodeId, Place,
};

/// A sequence inside the document that ops splice into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seq {
    /// The document's block list.
    Blocks,
    /// The body of an environment (by its block id).
    Body(NodeId),
    /// The inline list of a paragraph or heading, an environment's title
    /// (by block id), or the body of an inline call (by its id).
    Inlines(NodeId),
    /// The nodes of a math row (by row id).
    Row(NodeId),
    /// The chars of a text inline (by inline id) or raw-Typst block (by block id).
    Text(NodeId),
    /// The chars of the document header.
    Header,
    /// The chars of a block's label (by block id).
    Label(NodeId),
    /// The chars of the options of a math call or picture (by node or block id).
    Opts(NodeId),
}

impl Place {
    /// The sequence this place is in.
    pub fn seq(&self) -> Seq {
        match self.parent {
            None => Seq::Blocks,
            Some(env) => Seq::Body(env),
        }
    }
}

/// Content for a [`Seq`]. Text indices and counts are in chars.
#[derive(Clone, Debug, PartialEq)]
pub enum Items {
    Blocks(Vec<BlockNode>),
    Inlines(Vec<InlineNode>),
    Math(Vec<MathNode>),
    Text(String),
}

impl Items {
    pub fn len(&self) -> usize {
        match self {
            Items::Blocks(v) => v.len(),
            Items::Inlines(v) => v.len(),
            Items::Math(v) => v.len(),
            Items::Text(s) => s.chars().count(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Insert {
        seq: Seq,
        index: usize,
        items: Items,
    },
    Delete {
        seq: Seq,
        index: usize,
        count: usize,
    },
    Replace {
        seq: Seq,
        index: usize,
        count: usize,
        items: Items,
    },
    /// Move `count` nodes of `row` starting at `index` into the empty child
    /// row `slot` of `wrapper`, and put `wrapper` in their place.
    Wrap {
        row: NodeId,
        index: usize,
        count: usize,
        wrapper: MathNode,
        slot: Step,
    },
    /// Replace the structure at `row[index]` by the contents of its child
    /// row `slot`. The inverse is a `Wrap` that restores the other children.
    Unwrap {
        row: NodeId,
        index: usize,
        slot: Step,
    },
    /// Put a project file at `path`, or remove it (`None`). The inverse
    /// puts back what was there.
    SetFile {
        path: String,
        bytes: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpError {
    NotFound(Seq),
    OutOfRange {
        len: usize,
        index: usize,
        count: usize,
    },
    KindMismatch,
    BadWrapper,
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OpError::NotFound(seq) => write!(f, "sequence not found: {seq:?}"),
            OpError::OutOfRange { len, index, count } => {
                write!(f, "range {index}+{count} out of bounds for length {len}")
            }
            OpError::KindMismatch => write!(f, "items do not match the sequence kind"),
            OpError::BadWrapper => write!(f, "wrapper has no empty child at the given slot"),
        }
    }
}

impl std::error::Error for OpError {}

enum SeqMut<'a> {
    Blocks(&'a mut Vec<BlockNode>),
    Inlines(&'a mut Vec<InlineNode>),
    Row(&'a mut Vec<MathNode>),
    Text(&'a mut String),
}

impl SeqMut<'_> {
    fn len(&self) -> usize {
        match self {
            SeqMut::Blocks(v) => v.len(),
            SeqMut::Inlines(v) => v.len(),
            SeqMut::Row(v) => v.len(),
            SeqMut::Text(s) => s.chars().count(),
        }
    }

    fn empty_items(&self) -> Items {
        match self {
            SeqMut::Blocks(_) => Items::Blocks(vec![]),
            SeqMut::Inlines(_) => Items::Inlines(vec![]),
            SeqMut::Row(_) => Items::Math(vec![]),
            SeqMut::Text(_) => Items::Text(String::new()),
        }
    }

    /// Replace `index..index+count` by `items`, returning the removed items.
    /// Validates everything before mutating.
    fn splice(self, index: usize, count: usize, items: Items) -> Result<Items, OpError> {
        let len = self.len();
        if index.checked_add(count).is_none_or(|end| end > len) {
            return Err(OpError::OutOfRange { len, index, count });
        }
        let range = index..index + count;
        Ok(match (self, items) {
            (SeqMut::Blocks(v), Items::Blocks(new)) => {
                Items::Blocks(v.splice(range, new).collect())
            }
            (SeqMut::Inlines(v), Items::Inlines(new)) => {
                Items::Inlines(v.splice(range, new).collect())
            }
            (SeqMut::Row(v), Items::Math(new)) => Items::Math(v.splice(range, new).collect()),
            (SeqMut::Text(s), Items::Text(new)) => {
                let bytes = char_to_byte(s, index)..char_to_byte(s, index + count);
                let removed = s[bytes.clone()].to_owned();
                s.replace_range(bytes, &new);
                Items::Text(removed)
            }
            _ => return Err(OpError::KindMismatch),
        })
    }
}

/// The chars of text inline or raw block `id`, in any block at any depth.
fn text_in(doc: &mut Document, id: NodeId) -> Result<SeqMut<'_>, OpError> {
    if doc.block(id).is_some() {
        return match &mut doc.block_mut(id).expect("found").kind {
            Block::RawTypst(s)
            | Block::Diagram(s)
            | Block::Bibliography(s)
            | Block::Title(s)
            | Block::Image { path: s, .. } => Ok(SeqMut::Text(s)),
            _ => Err(OpError::KindMismatch),
        };
    }
    if let Some((row, k)) = doc.find_node(id).map(|(r, k)| (r.id, k)) {
        return match doc.find_row_mut(row).map(|r| &mut r.nodes[k].kind) {
            Some(MathKind::Code(s) | MathKind::Text(s)) => Ok(SeqMut::Text(s)),
            _ => Err(OpError::KindMismatch),
        };
    }
    match doc.inline_mut(id).map(|i| &mut i.kind) {
        Some(Inline::Text(s) | Inline::Raw(s) | Inline::Code(s)) => Ok(SeqMut::Text(s)),
        Some(_) => Err(OpError::KindMismatch),
        None => Err(OpError::NotFound(Seq::Text(id))),
    }
}

/// Byte offset of char index `ch` (which may equal the char count).
pub(crate) fn char_to_byte(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(b, _)| b)
}

impl Document {
    fn seq_mut(&mut self, seq: &Seq) -> Result<SeqMut<'_>, OpError> {
        let not_found = || OpError::NotFound(seq.clone());
        match *seq {
            Seq::Blocks => Ok(SeqMut::Blocks(&mut self.blocks)),
            Seq::Body(env) => self
                .block_mut(env)
                .and_then(|b| b.kind.body_mut())
                .map(SeqMut::Blocks)
                .ok_or_else(not_found),
            Seq::Header => Ok(SeqMut::Text(&mut self.header)),
            Seq::Inlines(owner) => self
                .inlines_of_mut(owner)
                .map(SeqMut::Inlines)
                .ok_or_else(not_found),
            Seq::Row(row) => self
                .find_row_mut(row)
                .map(|r| SeqMut::Row(&mut r.nodes))
                .ok_or_else(not_found),
            Seq::Text(id) => text_in(self, id),
            Seq::Label(block) => self
                .block_mut(block)
                .map(|b| SeqMut::Text(&mut b.label))
                .ok_or_else(not_found),
            Seq::Opts(id) if self.block(id).is_some() => {
                match &mut self.block_mut(id).ok_or_else(not_found)?.kind {
                    Block::Image { opts, .. } => Ok(SeqMut::Text(opts)),
                    _ => Err(OpError::KindMismatch),
                }
            }
            Seq::Opts(node) => {
                let (row, k) = self
                    .find_node(node)
                    .map(|(r, k)| (r.id, k))
                    .ok_or_else(not_found)?;
                match &mut self.find_row_mut(row).ok_or_else(not_found)?.nodes[k].kind {
                    MathKind::Call { opts, .. } => Ok(SeqMut::Text(opts)),
                    _ => Err(OpError::KindMismatch),
                }
            }
        }
    }

    /// Apply `op` and return its inverse. On error the document is unchanged.
    /// Remarks follow the edit.
    pub fn apply(&mut self, op: Op) -> Result<Op, OpError> {
        let edit = self.edit_of(&op);
        let inverse = self.apply_op(op)?;
        if let Some(edit) = edit {
            self.move_remarks(&edit);
        }
        Ok(inverse)
    }

    /// What `op` will change, for the remarks (before it is applied).
    fn edit_of(&self, op: &Op) -> Option<Edit> {
        let in_seq = |seq: &Seq, index: usize, removed: usize, inserted: usize| {
            let (node, drops) = match *seq {
                Seq::Text(id) => (Some(id), false),
                Seq::Row(id) => (Some(id), removed > 0),
                Seq::Blocks | Seq::Body(_) | Seq::Inlines(_) => (None, removed > 0),
                Seq::Header | Seq::Label(_) | Seq::Opts(_) => return None,
            };
            Some(Edit {
                node,
                index,
                removed,
                inserted,
                drops,
            })
        };
        match op {
            Op::Insert { seq, index, items } => in_seq(seq, *index, 0, items.len()),
            Op::Delete { seq, index, count } => in_seq(seq, *index, *count, 0),
            Op::Replace {
                seq,
                index,
                count,
                items,
            } => in_seq(seq, *index, *count, items.len()),
            // The wrapped nodes keep their ids: nothing leaves.
            Op::Wrap {
                row, index, count, ..
            } => Some(Edit {
                node: Some(*row),
                index: *index,
                removed: *count,
                inserted: 1,
                drops: false,
            }),
            Op::Unwrap { row, index, slot } => {
                let n = self
                    .find_row(*row)
                    .and_then(|r| r.nodes.get(*index))
                    .and_then(|w| w.child(*slot))
                    .map_or(0, |r| r.nodes.len());
                Some(Edit {
                    node: Some(*row),
                    index: *index,
                    removed: 1,
                    inserted: n,
                    drops: true,
                })
            }
            Op::SetFile { .. } => None,
        }
    }

    fn apply_op(&mut self, op: Op) -> Result<Op, OpError> {
        match op {
            Op::SetFile { path, bytes } => {
                let old = self.files.get(&path).map(<[u8]>::to_vec);
                match bytes {
                    Some(bytes) => self.files.insert(path.clone(), bytes),
                    None => self.files.remove(&path),
                }
                Ok(Op::SetFile { path, bytes: old })
            }
            Op::Insert { seq, index, items } => {
                let count = items.len();
                self.seq_mut(&seq)?.splice(index, 0, items)?;
                Ok(Op::Delete { seq, index, count })
            }
            Op::Delete { seq, index, count } => {
                let target = self.seq_mut(&seq)?;
                let empty = target.empty_items();
                let items = target.splice(index, count, empty)?;
                Ok(Op::Insert { seq, index, items })
            }
            Op::Replace {
                seq,
                index,
                count,
                items,
            } => {
                let new_count = items.len();
                let items = self.seq_mut(&seq)?.splice(index, count, items)?;
                Ok(Op::Replace {
                    seq,
                    index,
                    count: new_count,
                    items,
                })
            }
            Op::Wrap {
                row,
                index,
                count,
                mut wrapper,
                slot,
            } => {
                if !wrapper.child(slot).is_some_and(|r| r.nodes.is_empty()) {
                    return Err(OpError::BadWrapper);
                }
                let nodes = &mut self
                    .find_row_mut(row)
                    .ok_or(OpError::NotFound(Seq::Row(row)))?
                    .nodes;
                if index.checked_add(count).is_none_or(|end| end > nodes.len()) {
                    return Err(OpError::OutOfRange {
                        len: nodes.len(),
                        index,
                        count,
                    });
                }
                let moved: Vec<_> = nodes.splice(index..index + count, []).collect();
                wrapper.child_mut(slot).expect("checked above").nodes = moved;
                nodes.insert(index, wrapper);
                Ok(Op::Unwrap { row, index, slot })
            }
            Op::Unwrap { row, index, slot } => {
                let nodes = &mut self
                    .find_row_mut(row)
                    .ok_or(OpError::NotFound(Seq::Row(row)))?
                    .nodes;
                if !nodes.get(index).is_some_and(|n| n.child(slot).is_some()) {
                    return Err(OpError::BadWrapper);
                }
                let mut wrapper = nodes.remove(index);
                let inner =
                    std::mem::take(&mut wrapper.child_mut(slot).expect("checked above").nodes);
                let count = inner.len();
                nodes.splice(index..index, inner);
                Ok(Op::Wrap {
                    row,
                    index,
                    count,
                    wrapper,
                    slot,
                })
            }
        }
    }

    /// Apply a sequence of ops atomically. Returns the inverse sequence (to be
    /// applied in the returned order). On error everything is rolled back.
    pub fn apply_all(&mut self, ops: Vec<Op>) -> Result<Vec<Op>, OpError> {
        let mut inverses = Vec::with_capacity(ops.len());
        for op in ops {
            match self.apply(op) {
                Ok(inv) => inverses.push(inv),
                Err(e) => {
                    for inv in inverses.into_iter().rev() {
                        self.apply(inv).expect("inverse of an applied op applies");
                    }
                    return Err(e);
                }
            }
        }
        inverses.reverse();
        Ok(inverses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{MathKind, MathRow};

    fn sample() -> Document {
        let mut doc = Document::empty();
        let a = doc.text("héllo ");
        let x = doc.ident("x");
        let plus = doc.sym("+");
        let one = doc.num("1");
        let b = doc.ident("b");
        let f = doc.frac(vec![one], vec![b]);
        let m = doc.math(vec![x, plus, f]);
        let c = doc.text(" wörld");
        let p = doc.paragraph(vec![a, m, c]);
        let y = doc.ident("y");
        let d = doc.display(vec![y]);
        let r = doc.raw("#lorem(3)");
        doc.blocks = vec![p, d, r];
        doc
    }

    fn ids(doc: &Document) -> (NodeId, NodeId, NodeId, NodeId) {
        let p = &doc.blocks[0];
        let inl = p.kind.inlines().unwrap();
        let Inline::Math(row) = &inl[1].kind else {
            panic!()
        };
        (p.id, inl[0].id, row.id, doc.blocks[2].id)
    }

    fn roundtrip(doc: &Document, op: Op) {
        let mut d = doc.clone();
        let inv = d
            .apply(op.clone())
            .unwrap_or_else(|e| panic!("{op:?}: {e}"));
        let after = d.clone();
        let inv2 = d.apply(inv).unwrap();
        assert_eq!(&d, doc, "inverse restores");
        d.apply(inv2).unwrap();
        assert_eq!(d, after, "inverse of inverse redoes");
    }

    #[test]
    fn every_op_roundtrips() {
        let doc = sample();
        let (pid, tid, rid, rawid) = ids(&doc);
        let mut gen_doc = doc.clone();
        let z = gen_doc.ident("z");
        let para = gen_doc.paragraph(vec![]);
        let empty_text = gen_doc.text("");
        let wrapper = gen_doc.frac(vec![], vec![]);
        let script = gen_doc.scripts(vec![], Some(vec![]), None);

        let ops = [
            Op::Insert {
                seq: Seq::Text(tid),
                index: 1,
                items: Items::Text("ß€".into()),
            },
            Op::Insert {
                seq: Seq::Text(rawid),
                index: 9,
                items: Items::Text(" x".into()),
            },
            Op::Insert {
                seq: Seq::Row(rid),
                index: 3,
                items: Items::Math(vec![z.clone()]),
            },
            Op::Insert {
                seq: Seq::Blocks,
                index: 1,
                items: Items::Blocks(vec![para.clone()]),
            },
            Op::Insert {
                seq: Seq::Inlines(pid),
                index: 3,
                items: Items::Inlines(vec![empty_text]),
            },
            Op::Delete {
                seq: Seq::Text(tid),
                index: 1,
                count: 3,
            },
            Op::Delete {
                seq: Seq::Row(rid),
                index: 0,
                count: 3,
            },
            Op::Delete {
                seq: Seq::Blocks,
                index: 0,
                count: 2,
            },
            Op::Delete {
                seq: Seq::Inlines(pid),
                index: 1,
                count: 2,
            },
            Op::Replace {
                seq: Seq::Row(rid),
                index: 1,
                count: 2,
                items: Items::Math(vec![z]),
            },
            Op::Replace {
                seq: Seq::Text(tid),
                index: 0,
                count: 6,
                items: Items::Text("ä".into()),
            },
            Op::Replace {
                seq: Seq::Blocks,
                index: 2,
                count: 1,
                items: Items::Blocks(vec![para]),
            },
            Op::Wrap {
                row: rid,
                index: 0,
                count: 2,
                wrapper: wrapper.clone(),
                slot: Step::Num,
            },
            Op::Wrap {
                row: rid,
                index: 0,
                count: 0,
                wrapper,
                slot: Step::Den,
            },
            Op::Wrap {
                row: rid,
                index: 0,
                count: 1,
                wrapper: script,
                slot: Step::Base,
            },
            Op::Unwrap {
                row: rid,
                index: 2,
                slot: Step::Den,
            },
        ];
        for op in ops {
            roundtrip(&doc, op);
        }
    }

    #[test]
    fn errors_leave_document_unchanged() {
        let doc = sample();
        let (_, tid, rid, _) = ids(&doc);
        let mut d = doc.clone();
        let bad = [
            Op::Delete {
                seq: Seq::Text(tid),
                index: 5,
                count: 9,
            },
            Op::Insert {
                seq: Seq::Row(NodeId(9999)),
                index: 0,
                items: Items::Math(vec![]),
            },
            Op::Insert {
                seq: Seq::Row(rid),
                index: 0,
                items: Items::Text("x".into()),
            },
            Op::Unwrap {
                row: rid,
                index: 0,
                slot: Step::Num,
            },
        ];
        for op in bad {
            assert!(d.apply(op).is_err());
            assert_eq!(d, doc);
        }
        let mixed = vec![
            Op::Insert {
                seq: Seq::Text(tid),
                index: 0,
                items: Items::Text("ok".into()),
            },
            Op::Delete {
                seq: Seq::Row(rid),
                index: 9,
                count: 1,
            },
        ];
        assert!(d.apply_all(mixed).is_err());
        assert_eq!(d, doc);
    }

    #[test]
    fn unwrap_inverse_restores_other_children() {
        let doc = sample();
        let (_, _, rid, _) = ids(&doc);
        let mut d = doc.clone();
        let inv = d
            .apply(Op::Unwrap {
                row: rid,
                index: 2,
                slot: Step::Num,
            })
            .unwrap();
        let row: &MathRow = d.find_row(rid).unwrap();
        assert!(matches!(row.nodes[2].kind, MathKind::Num(_)));
        d.apply(inv).unwrap();
        assert_eq!(d, doc);
    }
}
