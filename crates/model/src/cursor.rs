//! Structural cursor: a path of steps from a block down to a text or a math
//! row, plus an offset in it. Never a source byte offset.
//!
//! Path shapes:
//! - Paragraph/heading: `[Index(i)]` selects inline `i`. For a text inline the
//!   path ends there and `offset` counts chars. For a math inline the path
//!   continues as a row path.
//! - Display math: the path is a row path from the block's row.
//! - Raw Typst: empty path, `offset` counts chars.
//! - Environment: empty path, `offset` 0 is just before it, 1 just after it
//!   (outside it; inside, the caret is in one of its blocks).
//! - Row path: pairs `Index(k), <child>` descending into node `k`'s child row
//!   (`Num`, `Den`, `Base`, `Sup`, `Sub`); `offset` is a position `0..=len`
//!   between the row's nodes.

use serde::{Deserialize, Serialize};

use crate::tree::{Block, BlockNode, Document, Inline, InlineNode, MathRow, NodeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Step {
    Index(usize),
    Num,
    Den,
    Base,
    Sup,
    Sub,
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::Index(_) => "item",
            Step::Num => "numerator",
            Step::Den => "denominator",
            Step::Base => "base",
            Step::Sup => "superscript",
            Step::Sub => "subscript",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CursorPath {
    pub block: NodeId,
    pub path: Vec<Step>,
    pub offset: usize,
}

/// The container a cursor points into.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Text {
        inline: &'a InlineNode,
        text: &'a str,
    },
    /// A raw Typst or diagram block (source edited as a whole).
    Raw {
        block: &'a BlockNode,
        text: &'a str,
    },
    Row {
        row: &'a MathRow,
    },
    /// Just before (offset 0) or after (offset 1) an environment.
    Edge {
        block: &'a BlockNode,
    },
}

impl Target<'_> {
    /// Number of caret positions minus one: chars for text, nodes for rows.
    pub fn len(&self) -> usize {
        match self {
            Target::Text { text, .. } | Target::Raw { text, .. } => text.chars().count(),
            Target::Row { row } => row.nodes.len(),
            Target::Edge { .. } => 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One breadcrumb entry for the status bar. `path_len` is the length of the
/// cursor path prefix that leads to this structure (for "select this").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub label: &'static str,
    pub path_len: usize,
}

impl Document {
    /// Resolve the container of a path in `block`. On failure returns the
    /// length of the longest valid prefix that leads to a container (or 0).
    pub fn walk<'a>(&'a self, block: &'a BlockNode, path: &[Step]) -> Result<Target<'a>, usize> {
        match &block.kind {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
                let Some(Step::Index(i)) = path.first() else {
                    return Err(0);
                };
                let inline = inlines.get(*i).ok_or(0usize)?;
                match &inline.kind {
                    Inline::Text(text) if path.len() == 1 => Ok(Target::Text { inline, text }),
                    Inline::Text(_) => Err(1),
                    Inline::Math(row) => walk_row(row, &path[1..]).map_err(|n| n + 1),
                }
            }
            Block::DisplayMath(row) => walk_row(row, path),
            Block::RawTypst(text) | Block::Diagram(text) if path.is_empty() => {
                Ok(Target::Raw { block, text })
            }
            Block::Env { .. } if path.is_empty() => Ok(Target::Edge { block }),
            Block::RawTypst(_) | Block::Diagram(_) | Block::Env { .. } => Err(0),
        }
    }

    pub fn resolve(&self, cursor: &CursorPath) -> Option<Target<'_>> {
        self.walk(self.block(cursor.block)?, &cursor.path).ok()
    }

    pub fn is_valid(&self, cursor: &CursorPath) -> bool {
        self.resolve(cursor)
            .is_some_and(|t| cursor.offset <= t.len())
    }

    /// The first caret position of `block` (of an environment: just before
    /// it).
    pub fn start_of(&self, block: &BlockNode) -> CursorPath {
        let path = if block.kind.inlines().is_some() {
            vec![Step::Index(0)]
        } else {
            vec![]
        };
        CursorPath {
            block: block.id,
            path,
            offset: 0,
        }
    }

    /// The last caret position of `block` (of an environment: just after
    /// it).
    pub fn end_of(&self, block: &BlockNode) -> CursorPath {
        let path = match block.kind.inlines() {
            Some(inlines) => vec![Step::Index(inlines.len().saturating_sub(1))],
            None => vec![],
        };
        let offset = self.walk(block, &path).map(|t| t.len()).unwrap_or(0);
        CursorPath {
            block: block.id,
            path,
            offset,
        }
    }

    /// The first and last caret positions of the document.
    pub fn doc_start(&self) -> CursorPath {
        self.start_of(&self.blocks[0])
    }

    pub fn doc_end(&self) -> CursorPath {
        self.end_of(self.blocks.last().expect("a document has a block"))
    }

    /// Repair a cursor that may have been invalidated by an edit: keep the
    /// longest valid path prefix and clamp the offset.
    pub fn fix_cursor(&self, cursor: &CursorPath) -> CursorPath {
        let Some(block) = self.block(cursor.block) else {
            return self.doc_start();
        };
        match self.walk(block, &cursor.path) {
            Ok(t) => CursorPath {
                offset: cursor.offset.min(t.len()),
                ..cursor.clone()
            },
            Err(n) => {
                let path = cursor.path[..n].to_vec();
                let hint = match cursor.path.get(n) {
                    Some(Step::Index(k)) => *k,
                    _ => cursor.offset,
                };
                match self.walk(block, &path) {
                    Ok(t) => CursorPath {
                        block: block.id,
                        path,
                        offset: hint.min(t.len()),
                    },
                    Err(_) if hint > 0 => self.end_of(block),
                    Err(_) => self.start_of(block),
                }
            }
        }
    }

    /// Every caret position of a block in reading order: the chars of each
    /// text, and in math rows each position between nodes, with the rows of
    /// a structure visited right after the position before it. ←/→ step
    /// through this list.
    pub fn positions(&self, block: &BlockNode) -> Vec<CursorPath> {
        let mut out = vec![];
        let at = |path: Vec<Step>, offset| CursorPath {
            block: block.id,
            path,
            offset,
        };
        match &block.kind {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
                for (i, inline) in inlines.iter().enumerate() {
                    match &inline.kind {
                        Inline::Text(s) => {
                            for k in 0..=s.chars().count() {
                                out.push(at(vec![Step::Index(i)], k));
                            }
                        }
                        Inline::Math(row) => {
                            row_positions(block.id, vec![Step::Index(i)], row, &mut out)
                        }
                    }
                }
            }
            Block::DisplayMath(row) => row_positions(block.id, vec![], row, &mut out),
            Block::RawTypst(s) | Block::Diagram(s) => {
                out.push(at(vec![], 0));
                out.push(at(vec![], s.chars().count()));
            }
            Block::Env { body, .. } => {
                out.push(at(vec![], 0));
                for child in body {
                    out.extend(self.positions(child));
                }
                out.push(at(vec![], 1));
            }
        }
        out
    }

    /// Labels for the status bar, e.g. paragraph › math › fraction › numerator.
    pub fn breadcrumb(&self, cursor: &CursorPath) -> Vec<Crumb> {
        let Some(block) = self.block(cursor.block) else {
            return vec![];
        };
        let mut crumbs = vec![Crumb {
            label: block.label(),
            path_len: 0,
        }];
        let mut path = &cursor.path[..];
        let mut consumed = 0;
        let mut row = match &block.kind {
            Block::DisplayMath(row) => row,
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
                let Some(Step::Index(i)) = path.first() else {
                    return crumbs;
                };
                let Some(Inline::Math(row)) = inlines.get(*i).map(|n| &n.kind) else {
                    return crumbs;
                };
                crumbs.push(Crumb {
                    label: "math",
                    path_len: 1,
                });
                path = &path[1..];
                consumed = 1;
                row
            }
            Block::RawTypst(_) | Block::Diagram(_) | Block::Env { .. } => return crumbs,
        };
        while let [Step::Index(k), child, rest @ ..] = path {
            let Some(node) = row.nodes.get(*k) else { break };
            let Some(next) = node.child(*child) else {
                break;
            };
            crumbs.push(Crumb {
                label: node.label(),
                path_len: consumed + 1,
            });
            crumbs.push(Crumb {
                label: child.label(),
                path_len: consumed + 2,
            });
            consumed += 2;
            path = rest;
            row = next;
        }
        crumbs
    }
}

fn row_positions(block: NodeId, path: Vec<Step>, row: &MathRow, out: &mut Vec<CursorPath>) {
    for (k, node) in row.nodes.iter().enumerate() {
        out.push(CursorPath {
            block,
            path: path.clone(),
            offset: k,
        });
        for (step, child) in node.children() {
            let mut p = path.clone();
            p.extend([Step::Index(k), step]);
            row_positions(block, p, child, out);
        }
    }
    out.push(CursorPath {
        block,
        path,
        offset: row.nodes.len(),
    });
}

fn walk_row<'a>(row: &'a MathRow, path: &[Step]) -> Result<Target<'a>, usize> {
    match path {
        [] => Ok(Target::Row { row }),
        [Step::Index(k), child, rest @ ..] => {
            let child = row
                .nodes
                .get(*k)
                .and_then(|n| n.child(*child))
                .ok_or(0usize)?;
            walk_row(child, rest).map_err(|n| n + 2)
        }
        _ => Err(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "a $x + (1)/(b^2)$ c" and a display formula.
    pub(crate) fn sample() -> Document {
        let mut doc = Document::empty();
        let a = doc.text("a ");
        let x = doc.ident("x");
        let plus = doc.sym("+");
        let one = doc.num("1");
        let b = doc.ident("b");
        let two = doc.num("2");
        let b2 = doc.scripts(vec![b], Some(vec![two]), None);
        let f = doc.frac(vec![one], vec![b2]);
        let m = doc.math(vec![x, plus, f]);
        let c = doc.text(" c");
        let p = doc.paragraph(vec![a, m, c]);
        let y = doc.ident("y");
        let d = doc.display(vec![y]);
        doc.blocks = vec![p, d];
        doc
    }

    #[test]
    fn resolve_and_breadcrumb() {
        let doc = sample();
        let block = doc.blocks[0].id;
        let c = CursorPath {
            block,
            path: vec![
                Step::Index(1),
                Step::Index(2),
                Step::Den,
                Step::Index(0),
                Step::Sup,
            ],
            offset: 1,
        };
        assert!(doc.is_valid(&c));
        let labels: Vec<_> = doc.breadcrumb(&c).iter().map(|c| c.label).collect();
        assert_eq!(
            labels,
            [
                "paragraph",
                "math",
                "fraction",
                "denominator",
                "scripts",
                "superscript"
            ]
        );
        assert!(!doc.is_valid(&CursorPath {
            offset: 2,
            ..c.clone()
        }));
        let text = CursorPath {
            block,
            path: vec![Step::Index(2)],
            offset: 2,
        };
        assert!(doc.is_valid(&text));
    }

    #[test]
    fn fix_cursor_clamps_and_truncates() {
        let doc = sample();
        let block = doc.blocks[0].id;
        let bad = CursorPath {
            block,
            path: vec![
                Step::Index(1),
                Step::Index(2),
                Step::Sup,
                Step::Index(0),
                Step::Num,
            ],
            offset: 0,
        };
        let fixed = doc.fix_cursor(&bad);
        assert_eq!(fixed.path, vec![Step::Index(1)]);
        assert_eq!(fixed.offset, 2);
        let far = CursorPath {
            block,
            path: vec![Step::Index(9)],
            offset: 0,
        };
        assert_eq!(doc.fix_cursor(&far), doc.end_of(&doc.blocks[0]));
        let gone = CursorPath {
            block: NodeId(999),
            path: vec![],
            offset: 0,
        };
        assert_eq!(doc.fix_cursor(&gone), doc.doc_start());
    }
}
