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
//!   (outside it; inside, the caret is in one of its blocks). Its title is
//!   addressed like a paragraph's inlines, `[Index(i)]`.
//! - List item: no position of its own; a term item's term is addressed
//!   like a paragraph's inlines.
//! - Inline call: `Index(i), Body` descends into the body of inline call
//!   `i`, addressed like a paragraph's inlines (`[Index(1), Body, Index(0)]`
//!   is the first text of the paragraph's first call).
//! - Label: `[Label]` in a labelable block, `offset` counts chars of its
//!   label (the pill beside the page). Its positions come after the block's
//!   own (an environment's: before the position just after it).
//! - Row path: pairs `Index(k), <child>` descending into node `k`'s child row
//!   (`Num`, `Den`, `Base`, `Sup`, `Sub`, `Arg(i)`); `offset` is a position `0..=len`
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
    /// Argument `i` of a function call (`sqrt(x)`, `root(3, x)`).
    Arg(usize),
    /// The body of an inline call (`*bold*`, `#cl[…]`).
    Body,
    /// The label of a block (`[Label]`).
    Label,
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
            Step::Arg(_) => "argument",
            Step::Body => "body",
            Step::Label => "label",
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
    /// The label of a block.
    Label {
        block: &'a BlockNode,
        text: &'a str,
    },
}

impl Target<'_> {
    /// Number of caret positions minus one: chars for text, nodes for rows.
    pub fn len(&self) -> usize {
        match self {
            Target::Text { text, .. } | Target::Raw { text, .. } | Target::Label { text, .. } => {
                text.chars().count()
            }
            Target::Row { row } => row.nodes.len(),
            Target::Edge { .. } => 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Where a path meets an inline list: the list, its owner (a block, or an
/// inline call for its body) and the index of the inline the path goes on
/// into. `prefix` is the length of the path before that `Index`.
#[derive(Clone, Copy, Debug)]
pub struct InlineAt<'a> {
    pub owner: NodeId,
    pub inlines: &'a [InlineNode],
    pub prefix: usize,
    pub index: usize,
}

/// The innermost inline list `path` goes through, from `inlines` owned by
/// `owner`; descends into calls while the path goes into their body.
fn inline_at<'a>(
    owner: NodeId,
    inlines: &'a [InlineNode],
    path: &[Step],
    prefix: usize,
) -> Option<InlineAt<'a>> {
    let Some(Step::Index(i)) = path.first() else {
        return None;
    };
    let inline = inlines.get(*i)?;
    if let (Some(body), Some(Step::Body)) = (inline.body(), path.get(1)) {
        return inline_at(inline.id, body, &path[2..], prefix + 2);
    }
    Some(InlineAt {
        owner,
        inlines,
        prefix,
        index: *i,
    })
}

fn walk_inlines<'a>(inlines: &'a [InlineNode], path: &[Step]) -> Result<Target<'a>, usize> {
    let Some(Step::Index(i)) = path.first() else {
        return Err(0);
    };
    let inline = inlines.get(*i).ok_or(0usize)?;
    match &inline.kind {
        Inline::Text(text) | Inline::Raw(text) if path.len() == 1 => {
            Ok(Target::Text { inline, text })
        }
        Inline::Text(_) | Inline::Raw(_) => Err(1),
        // An atom: no position inside.
        Inline::Ref(_) => Err(0),
        Inline::Math(row) => walk_row(row, &path[1..]).map_err(|n| n + 1),
        Inline::Call { body, .. } => match path.get(1) {
            Some(Step::Body) => walk_inlines(body, &path[2..]).map_err(|n| n + 2),
            _ => Err(1),
        },
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
        if path == [Step::Label] && block.kind.labelable() {
            return Ok(Target::Label {
                block,
                text: &block.label,
            });
        }
        match &block.kind {
            Block::Env { .. } if path.is_empty() => Ok(Target::Edge { block }),
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines)
            | Block::Env { title: inlines, .. }
            | Block::Item { term: inlines, .. } => walk_inlines(inlines, path),
            Block::DisplayMath(row) => walk_row(row, path),
            Block::RawTypst(text)
            | Block::Diagram(text)
            | Block::Bibliography(text)
            | Block::Title(text)
                if path.is_empty() =>
            {
                Ok(Target::Raw { block, text })
            }
            Block::RawTypst(_) | Block::Diagram(_) | Block::Bibliography(_) | Block::Title(_) => {
                Err(0)
            }
        }
    }

    /// The innermost inline list of `block` that `path` goes through (see
    /// [`InlineAt`]); `None` outside text.
    pub fn inline_at<'a>(&'a self, block: &'a BlockNode, path: &[Step]) -> Option<InlineAt<'a>> {
        if matches!(block.kind, Block::Env { .. }) && path.is_empty() {
            return None;
        }
        inline_at(block.id, block.kind.text_inlines()?, path, 0)
    }

    /// The path to inline `id` in block `block` (through inline calls).
    pub fn inline_path(&self, block: NodeId, id: NodeId) -> Option<Vec<Step>> {
        fn find(inlines: &[InlineNode], id: NodeId, path: &mut Vec<Step>) -> bool {
            for (i, inline) in inlines.iter().enumerate() {
                path.push(Step::Index(i));
                if inline.id == id {
                    return true;
                }
                if let Some(body) = inline.body() {
                    path.push(Step::Body);
                    if find(body, id, path) {
                        return true;
                    }
                    path.pop();
                }
                path.pop();
            }
            false
        }
        let mut path = vec![];
        let inlines = self.block(block)?.kind.text_inlines()?;
        find(inlines, id, &mut path).then_some(path)
    }

    pub fn resolve(&self, cursor: &CursorPath) -> Option<Target<'_>> {
        self.walk(self.block(cursor.block)?, &cursor.path).ok()
    }

    pub fn is_valid(&self, cursor: &CursorPath) -> bool {
        self.resolve(cursor)
            .is_some_and(|t| cursor.offset <= t.len())
    }

    /// The first caret position of `block` (of an environment: just before
    /// it; of a list item: in its term or its first block).
    pub fn start_of(&self, block: &BlockNode) -> CursorPath {
        if let Block::Item { term, body, .. } = &block.kind {
            return match (term.is_empty(), body.first()) {
                (true, Some(first)) => self.start_of(first),
                _ => CursorPath {
                    block: block.id,
                    path: vec![Step::Index(0)],
                    offset: 0,
                },
            };
        }
        let path = if block.kind.own_inlines().is_some() {
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
        if let Some(last) = match &block.kind {
            Block::Item { body, .. } => body.last(),
            _ => None,
        } {
            return self.end_of(last);
        }
        let path = match block.kind.own_inlines() {
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
        fn inline_positions(
            block: NodeId,
            prefix: &[Step],
            inlines: &[InlineNode],
            out: &mut Vec<CursorPath>,
        ) {
            for (i, inline) in inlines.iter().enumerate() {
                let mut path = prefix.to_vec();
                path.push(Step::Index(i));
                match &inline.kind {
                    Inline::Text(s) | Inline::Raw(s) => {
                        for k in 0..=s.chars().count() {
                            out.push(CursorPath {
                                block,
                                path: path.clone(),
                                offset: k,
                            });
                        }
                    }
                    Inline::Math(row) => row_positions(block, path, row, out),
                    Inline::Call { body, .. } => {
                        path.push(Step::Body);
                        inline_positions(block, &path, body, out);
                    }
                    Inline::Ref(_) => {}
                }
            }
        }
        let inline_positions = |inlines: &[InlineNode], out: &mut Vec<CursorPath>| {
            inline_positions(block.id, &[], inlines, out)
        };
        let label = |out: &mut Vec<CursorPath>| {
            for k in 0..=block.label.chars().count() {
                out.push(at(vec![Step::Label], k));
            }
        };
        match &block.kind {
            Block::Paragraph(inlines) | Block::Caption(inlines) => {
                inline_positions(inlines, &mut out)
            }
            Block::Heading { inlines, .. } => {
                inline_positions(inlines, &mut out);
                label(&mut out);
            }
            Block::DisplayMath(row) => {
                row_positions(block.id, vec![], row, &mut out);
                label(&mut out);
            }
            Block::RawTypst(s) | Block::Diagram(s) | Block::Bibliography(s) | Block::Title(s) => {
                out.push(at(vec![], 0));
                out.push(at(vec![], s.chars().count()));
            }
            Block::Env { title, body, .. } => {
                out.push(at(vec![], 0));
                inline_positions(title, &mut out);
                for child in body {
                    out.extend(self.positions(child));
                }
                label(&mut out);
                out.push(at(vec![], 1));
            }
            Block::Item { term, body, .. } => {
                inline_positions(term, &mut out);
                for child in body {
                    out.extend(self.positions(child));
                }
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
        if cursor.path == [Step::Label] {
            crumbs.push(Crumb {
                label: "label",
                path_len: 0,
            });
            return crumbs;
        }
        let mut path = &cursor.path[..];
        let mut consumed = 0;
        let mut row = match &block.kind {
            Block::DisplayMath(row) => row,
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines)
            | Block::Env { title: inlines, .. }
            | Block::Item { term: inlines, .. } => {
                let Some(Step::Index(i)) = path.first() else {
                    return crumbs;
                };
                let label = match block.kind {
                    Block::Env { .. } => Some("title"),
                    Block::Item { .. } => Some("term"),
                    _ => None,
                };
                if let Some(label) = label {
                    crumbs.push(Crumb { label, path_len: 0 });
                }
                let mut inlines = inlines;
                let mut i = *i;
                // Into inline calls, then a formula.
                loop {
                    let Some(inline) = inlines.get(i) else {
                        return crumbs;
                    };
                    match (&inline.kind, path.get(1)) {
                        (Inline::Call { body, .. }, Some(Step::Body)) => {
                            crumbs.push(Crumb {
                                label: inline.label(),
                                path_len: consumed + 1,
                            });
                            path = &path[2..];
                            consumed += 2;
                            let Some(Step::Index(j)) = path.first() else {
                                return crumbs;
                            };
                            inlines = body;
                            i = *j;
                        }
                        (Inline::Math(_), _) => break,
                        (Inline::Raw(_), _) => {
                            crumbs.push(Crumb {
                                label: "code",
                                path_len: consumed + 1,
                            });
                            return crumbs;
                        }
                        _ => return crumbs,
                    }
                }
                let Some(Inline::Math(row)) = inlines.get(i).map(|n| &n.kind) else {
                    return crumbs;
                };
                crumbs.push(Crumb {
                    label: "math",
                    path_len: consumed + 1,
                });
                path = &path[1..];
                consumed += 1;
                row
            }
            Block::RawTypst(_) | Block::Diagram(_) | Block::Bibliography(_) | Block::Title(_) => {
                return crumbs;
            }
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
