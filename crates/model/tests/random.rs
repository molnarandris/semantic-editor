//! Randomized tests: every op round-trips through its inverse, invariants
//! hold, and cursors stay repairable after every op.

use model::*;

/// Small deterministic PRNG so the test needs no extra dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn atom(doc: &mut Document, rng: &mut Rng) -> MathNode {
    match rng.below(3) {
        0 => doc.ident(["x", "y", "chi", "α"][rng.below(4)]),
        1 => doc.num(["1", "2", "3.5"][rng.below(3)]),
        _ => doc.sym(["+", "=", "≤", ","][rng.below(4)]),
    }
}

fn math_nodes(doc: &mut Document, rng: &mut Rng, depth: u32) -> Vec<MathNode> {
    (0..rng.below(4))
        .map(|_| match rng.below(if depth == 0 { 1 } else { 4 }) {
            1 => {
                let (n, d) = (
                    math_nodes(doc, rng, depth - 1),
                    math_nodes(doc, rng, depth - 1),
                );
                doc.frac(n, d)
            }
            2 => {
                let b = vec![atom(doc, rng)];
                let s = math_nodes(doc, rng, depth - 1);
                doc.scripts(b, Some(s), None)
            }
            _ => atom(doc, rng),
        })
        .collect()
}

fn random_doc(rng: &mut Rng) -> Document {
    let mut doc = Document::empty();
    for _ in 0..1 + rng.below(4) {
        let block = match rng.below(3) {
            0 => {
                let n = math_nodes(&mut doc, rng, 3);
                doc.display(n)
            }
            _ => {
                let t1 = doc.text("some text ");
                let n = math_nodes(&mut doc, rng, 3);
                let m = doc.math(n);
                let t2 = doc.text(" and more");
                doc.paragraph(vec![t1, m, t2])
            }
        };
        doc.blocks.push(block);
    }
    doc
}

/// Every valid caret position in the document.
fn all_cursors(doc: &Document) -> Vec<CursorPath> {
    fn row(out: &mut Vec<CursorPath>, block: NodeId, path: Vec<Step>, r: &MathRow) {
        for offset in 0..=r.nodes.len() {
            out.push(CursorPath {
                block,
                path: path.clone(),
                offset,
            });
        }
        for (k, node) in r.nodes.iter().enumerate() {
            for (step, child) in node.children() {
                let mut p = path.clone();
                p.extend([Step::Index(k), step]);
                row(out, block, p, child);
            }
        }
    }
    let mut out = vec![];
    for b in doc.leaves() {
        match &b.kind {
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines) => {
                for (i, inline) in inlines.iter().enumerate() {
                    match &inline.kind {
                        Inline::Text(s) => (0..=s.chars().count()).for_each(|o| {
                            out.push(CursorPath {
                                block: b.id,
                                path: vec![Step::Index(i)],
                                offset: o,
                            })
                        }),
                        Inline::Math(r) => row(&mut out, b.id, vec![Step::Index(i)], r),
                        Inline::Call { .. } | Inline::Raw(_) | Inline::Ref(_) => {}
                    }
                }
            }
            Block::DisplayMath(r) => row(&mut out, b.id, vec![], r),
            Block::RawTypst(s) | Block::Diagram(s) => (0..=s.chars().count()).for_each(|o| {
                out.push(CursorPath {
                    block: b.id,
                    path: vec![],
                    offset: o,
                })
            }),
            Block::Env { .. } | Block::Item { .. } => unreachable!("leaves are not containers"),
        }
    }
    out
}

fn all_rows(doc: &Document) -> Vec<(NodeId, usize)> {
    fn walk(out: &mut Vec<(NodeId, usize)>, r: &MathRow) {
        out.push((r.id, r.nodes.len()));
        for n in &r.nodes {
            for (_, c) in n.children() {
                walk(out, c);
            }
        }
    }
    let mut out = vec![];
    for b in &doc.blocks {
        for r in b.rows() {
            walk(&mut out, r);
        }
    }
    out
}

fn random_op(doc: &mut Document, rng: &mut Rng) -> Op {
    let rows = all_rows(doc);
    let (row, len) = rows[rng.below(rows.len())];
    let index = rng.below(len + 1);
    let count = rng.below(len - index + 1);
    match rng.below(6) {
        0 => {
            let items = Items::Math(math_nodes(doc, rng, 2));
            Op::Insert {
                seq: Seq::Row(row),
                index,
                items,
            }
        }
        1 => Op::Delete {
            seq: Seq::Row(row),
            index,
            count,
        },
        2 => {
            let items = Items::Math(math_nodes(doc, rng, 2));
            Op::Replace {
                seq: Seq::Row(row),
                index,
                count,
                items,
            }
        }
        3 => {
            let (wrapper, slot) = if rng.below(2) == 0 {
                (
                    doc.frac(vec![], vec![]),
                    [Step::Num, Step::Den][rng.below(2)],
                )
            } else {
                (
                    doc.scripts(vec![], Some(vec![]), Some(vec![])),
                    [Step::Base, Step::Sup][rng.below(2)],
                )
            };
            Op::Wrap {
                row,
                index,
                count,
                wrapper,
                slot,
            }
        }
        4 => {
            // Text op on a random text inline, if any.
            let texts: Vec<_> = doc
                .blocks
                .iter()
                .flat_map(|b| b.kind.inlines().into_iter().flatten())
                .filter_map(|i| match &i.kind {
                    Inline::Text(s) => Some((i.id, s.chars().count())),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                return Op::Delete {
                    seq: Seq::Row(row),
                    index,
                    count: 0,
                };
            }
            let (id, n) = texts[rng.below(texts.len())];
            let index = rng.below(n + 1);
            let count = rng.below(n - index + 1);
            Op::Replace {
                seq: Seq::Text(id),
                index,
                count,
                items: Items::Text("ü+".into()),
            }
        }
        _ => {
            let row_ref = doc.find_row(row).unwrap();
            match row_ref.nodes.iter().position(|n| n.is_structure()) {
                Some(i) => {
                    let slot = row_ref.nodes[i].children()[rng.below(2)].0;
                    Op::Unwrap {
                        row,
                        index: i,
                        slot,
                    }
                }
                None => Op::Delete {
                    seq: Seq::Row(row),
                    index,
                    count,
                },
            }
        }
    }
}

#[test]
fn random_ops_roundtrip_and_keep_cursors_valid() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..200 {
        let mut doc = random_doc(&mut rng);
        doc.check_invariants().unwrap();
        let mut history = History::new();
        let mut snapshots = vec![doc.clone()];
        for _ in 0..15 {
            let cursors = all_cursors(&doc);
            let op = random_op(&mut doc, &mut rng);
            let before_op = doc.clone();
            let caret = cursors[rng.below(cursors.len())].clone();

            let mut scratch = doc.clone();
            let inv = scratch
                .apply(op.clone())
                .unwrap_or_else(|e| panic!("{op:?}: {e}"));
            scratch.check_invariants().unwrap();
            for c in &cursors {
                let fixed = scratch.fix_cursor(c);
                assert!(scratch.is_valid(&fixed), "{c:?} -> {fixed:?} after {op:?}");
            }
            scratch.apply(inv).unwrap();
            assert_eq!(scratch, before_op);

            history
                .apply(&mut doc, vec![op], caret.clone(), caret)
                .unwrap();
            snapshots.push(doc.clone());
        }
        // Undo everything, then redo everything.
        for snap in snapshots.iter().rev().skip(1) {
            history.undo(&mut doc).unwrap();
            assert_eq!(doc.blocks, snap.blocks); // next_id keeps counting: ids are never reused
        }
        assert!(!history.can_undo());
        for snap in snapshots.iter().skip(1) {
            history.redo(&mut doc).unwrap();
            assert_eq!(doc.blocks, snap.blocks); // next_id keeps counting: ids are never reused
        }
    }
}

/// Random keystrokes through the editor: invariants hold and the caret stays
/// valid after every command, and undoing everything restores the start.
#[test]
fn random_keystrokes_keep_the_document_sound() {
    let mut rng = Rng(0x5eed_cafe);
    let keys = [
        "a", "x", "chi", " ", "2", ".", "+", "<", "=", "-", ">", "/", "^", "_", "(", ")", "$",
        "\\al", "é ", "\\", "*", "#cl[", "]", "`", "--", "~", "@e", ". ",
    ];
    for round in 0..150 {
        let doc = random_doc(&mut rng);
        let start = doc.blocks.clone();
        let mut e = Editor::new(doc);
        for _ in 0..40 {
            match rng.below(16) {
                0..=5 => {
                    e.insert_text(keys[rng.below(keys.len())]);
                }
                6 => {
                    e.backspace();
                }
                7 => {
                    e.delete();
                }
                8 => {
                    e.move_left();
                }
                9 => {
                    e.move_right();
                }
                10 => {
                    e.escape();
                }
                11 => {
                    e.enter();
                }
                12 => {
                    e.next_slot(rng.below(2) == 0);
                }
                13 => {
                    let kind = [NewBlock::Diagram, NewBlock::RawTypst][rng.below(2)];
                    e.insert_block(kind);
                }
                14 => {
                    let block = e.caret().block;
                    e.set_block_source(block, ["circle((0, 0))", "", "x"][rng.below(3)]);
                }
                _ => {
                    if let Some(c) = e.vertical_target(rng.below(2) == 0) {
                        e.set_caret(c);
                    }
                }
            }
            e.doc().check_invariants().unwrap();
            assert!(e.doc().is_valid(e.caret()), "round {round}: bad caret");
            let _ = (e.selected_ids(), e.context_ids(), e.hint());
        }
        while e.undo() {}
        assert_eq!(e.doc().blocks, start, "round {round}: undo all");
    }
}
