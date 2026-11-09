//! Caret geometry: every caret position of a block with its place on the
//! rendered page. Built from the block's [`BlockLayout`]; no GTK here.
//!
//! Text positions come from glyph clusters (a ligature's width is split
//! evenly between its chars). Positions without a glyph (collapsed or
//! trailing spaces) are placed after the previous one. Math rows put a
//! position at the left edge of each node and after the last one, and the
//! middle of an empty slot; positions in a row span the row's height.

use model::{
    Block, BlockNode, CursorPath, Document, Inline, MathKind, MathNode, MathRow, Step, Target,
};
use typst_bridge::{BlockLayout, Rect};

/// Advance used for chars that produced no glyph, in pt (≈ a space at 11 pt).
const SPACE_PT: f64 = 3.7;
/// How far an environment's outline lies outside its box, in pt.
pub(crate) const OUTLINE_PAD_PT: f64 = 3.0;
/// The caret just before/after an environment sits this far outside its box
/// (outside the outline), in pt.
const EDGE_GAP_PT: f64 = OUTLINE_PAD_PT + 1.5;
/// Caret extent for a block without content, in pt.
const EMPTY_TOP_PT: f64 = 0.0;
const EMPTY_BOTTOM_PT: f64 = 11.0;
/// Two stops belong to the same line if their baselines differ less.
const LINE_TOLERANCE_PT: f64 = 2.0;

/// One caret position, in block coordinates (pt).
#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub cursor: CursorPath,
    pub x: f64,
    pub top: f64,
    pub bottom: f64,
    /// Baseline of the line, used to group stops into lines.
    pub line: f64,
    /// 0 in text, 1 in a formula's top row, +1 per structure level.
    pub depth: usize,
    /// Just before/after an environment (outside it): Home/End stay inside.
    pub edge: bool,
}

/// All caret positions of a block, in document order.
pub fn stops(block: &BlockNode, layout: Option<&BlockLayout>) -> Vec<Stop> {
    let mut out = layout.map_or(vec![], |l| block_stops(block, l, true));
    if out.is_empty() {
        // Not rendered yet, or no text (an empty paragraph or heading).
        let mut block = block;
        while let Some(first) = block.kind.body().and_then(|b| b.first()) {
            block = first;
        }
        let start = match block.kind.inlines() {
            Some(_) => vec![Step::Index(0)],
            None => vec![],
        };
        let baseline = layout.map_or(EMPTY_BOTTOM_PT * 0.8, |l| l.baseline.max(1.0));
        // An empty heading still shows its number: typed text goes after it.
        let x = layout
            .and_then(|l| l.box_of(block.id))
            .map_or(0.0, |r| r.right() + SPACE_PT);
        out.push(Stop {
            cursor: CursorPath {
                block: block.id,
                path: start,
                offset: 0,
            },
            x,
            top: EMPTY_TOP_PT,
            bottom: EMPTY_BOTTOM_PT,
            line: baseline,
            depth: 0,
            edge: false,
        });
    }
    out
}

fn stop(
    block: &BlockNode,
    path: Vec<Step>,
    offset: usize,
    x: f64,
    top: f64,
    bottom: f64,
    line: f64,
) -> Stop {
    Stop {
        cursor: CursorPath {
            block: block.id,
            path,
            offset,
        },
        x,
        top,
        bottom,
        line,
        depth: 0,
        edge: false,
    }
}

fn cursor(block: &BlockNode, path: Vec<Step>, offset: usize) -> CursorPath {
    CursorPath {
        block: block.id,
        path,
        offset,
    }
}

/// Stops of a math row and, recursively, of the rows inside its nodes.
fn row_stops(
    block: &BlockNode,
    path: Vec<Step>,
    row: &MathRow,
    l: &BlockLayout,
    line: f64,
    depth: usize,
    out: &mut Vec<Stop>,
) {
    let Some(rb) = l.box_of(row.id) else {
        return;
    };
    let (top, bottom) = (rb.y, rb.bottom());
    let at = |path: Vec<Step>, offset, x| Stop {
        depth,
        ..stop(block, path, offset, x, top, bottom, line)
    };
    if row.nodes.is_empty() {
        out.push(at(path, 0, rb.x + rb.w / 2.0));
        return;
    }
    // Lines of a multi-line formula (split at `\\`): each stop takes the
    // height of its line, and its baseline is moved down with the line. An
    // empty line is the placeholder written with the break before it.
    let breaks: Vec<&MathNode> = row
        .nodes
        .iter()
        .filter(|n| n.kind == MathKind::Linebreak)
        .collect();
    let lines: Vec<Rect> = row
        .nodes
        .split(|n| n.kind == MathKind::Linebreak)
        .enumerate()
        .map(|(i, nodes)| {
            nodes
                .iter()
                .filter_map(|n| l.box_of(n.id))
                .reduce(|a, b| a.union(&b))
                .or_else(|| i.checked_sub(1).and_then(|b| l.box_of(breaks[b].id)))
                .unwrap_or(rb)
        })
        .collect();
    let at_line = |path: Vec<Step>, offset, x, i: usize| {
        let r = lines[i];
        Stop {
            depth,
            ..stop(
                block,
                path,
                offset,
                x,
                r.y,
                r.bottom(),
                line + r.y - lines[0].y,
            )
        }
    };
    let mut i = 0;
    let mut right = rb.x;
    for (k, node) in row.nodes.iter().enumerate() {
        let brk = node.kind == MathKind::Linebreak;
        // A break's box is the placeholder of the empty line after it.
        let r = l.box_of(node.id).filter(|_| !brk);
        let x = r.map_or(right, |r| r.x);
        out.push(if lines.len() > 1 {
            at_line(path.clone(), k, x, i)
        } else {
            at(path.clone(), k, x)
        });
        if brk {
            i += 1;
            let empty = row
                .nodes
                .get(k + 1)
                .is_none_or(|n| n.kind == MathKind::Linebreak);
            right = if empty {
                lines[i].x + lines[i].w / 2.0
            } else {
                lines[i].x
            };
        }
        if let Some(r) = r {
            right = r.right();
        }
        for (step, child) in node.children() {
            let mut p = path.clone();
            p.extend([Step::Index(k), step]);
            row_stops(block, p, child, l, line, depth + 1, out);
        }
    }
    out.push(if lines.len() > 1 {
        at_line(path, row.nodes.len(), right, i)
    } else {
        at(path, row.nodes.len(), right)
    });
}

/// The stops of `block` in layout `l`: the whole rendered block (`top`), or
/// a block inside an environment, whose stops come from the environment's
/// layout.
fn block_stops(block: &BlockNode, l: &BlockLayout, top: bool) -> Vec<Stop> {
    match &block.kind {
        Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
            inline_stops(block, inlines, l)
        }
        Block::DisplayMath(row) => {
            let line = match (top, l.box_of(block.id)) {
                (false, Some(r)) => line_near(l, r),
                _ => l.baseline,
            };
            let mut v = vec![];
            row_stops(block, vec![], row, l, line, 1, &mut v);
            v
        }
        Block::RawTypst(text) | Block::Diagram(text) => {
            let n = text.chars().count();
            let r = match (top, l.box_of(block.id)) {
                (true, _) => Rect {
                    x: 0.0,
                    y: 0.0,
                    w: l.width,
                    h: l.height,
                },
                (false, Some(r)) => r,
                (false, None) => return vec![],
            };
            let line = if top { l.baseline } else { r.bottom() };
            vec![
                stop(block, vec![], 0, r.x, r.y, r.bottom(), line),
                stop(block, vec![], n, r.right(), r.y, r.bottom(), line),
            ]
        }
        Block::Item { term, body, .. } => {
            let mut out = inline_stops(block, term, l);
            out.extend(body.iter().flat_map(|b| block_stops(b, l, false)));
            out
        }
        Block::Env { title, body, .. } => {
            let mut inner = inline_stops(block, title, l);
            inner.extend(body.iter().flat_map(|b| block_stops(b, l, false)));
            let r = match (top, l.box_of(block.id)) {
                (true, _) => Rect {
                    x: 0.0,
                    y: 0.0,
                    w: l.width,
                    h: l.height,
                },
                (false, Some(r)) => r,
                (false, None) => return inner,
            };
            let (Some(first), Some(last)) = (inner.first(), inner.last()) else {
                return inner;
            };
            // Just before and after it, outside its outline: on its first and
            // last line.
            let edge = |offset, x, s: &Stop| Stop {
                edge: true,
                ..stop(block, vec![], offset, x, s.top, s.bottom, s.line)
            };
            let before = edge(0, r.x - EDGE_GAP_PT, first);
            let after = edge(1, r.right() + EDGE_GAP_PT, last);
            let mut out = vec![before];
            out.extend(inner);
            out.push(after);
            out
        }
    }
}

fn inline_stops(block: &BlockNode, inlines: &[model::InlineNode], l: &BlockLayout) -> Vec<Stop> {
    list_stops(block, &[], inlines, l)
}

/// The stops of an inline list at `prefix` (a block's, or an inline call's
/// body).
fn list_stops(
    block: &BlockNode,
    prefix: &[Step],
    inlines: &[model::InlineNode],
    l: &BlockLayout,
) -> Vec<Stop> {
    let at = |i: usize| {
        let mut p = prefix.to_vec();
        p.push(Step::Index(i));
        p
    };
    // Per text inline: stop by char offset.
    let mut per: Vec<Vec<Option<Stop>>> = inlines
        .iter()
        .map(|i| match &i.kind {
            Inline::Text(s) | Inline::Raw(s) => vec![None; s.chars().count() + 1],
            Inline::Math(_) | Inline::Call { .. } => vec![],
        })
        .collect();

    // Per formula or call: the stops inside it.
    let mut inner: Vec<Vec<Stop>> = vec![vec![]; inlines.len()];

    for (i, inline) in inlines.iter().enumerate() {
        let path = at(i);
        match &inline.kind {
            Inline::Call { body, .. } => {
                let mut p = path.clone();
                p.push(Step::Body);
                inner[i] = list_stops(block, &p, body, l);
                // Just before and after it: where its text starts and ends.
                let (first, last) = match (inner[i].first(), inner[i].last()) {
                    (Some(f), Some(e)) => (f.clone(), e.clone()),
                    _ => {
                        let Some(r) = l.box_of(inline.id) else {
                            continue;
                        };
                        let s = stop(block, vec![], 0, r.x, r.y, r.bottom(), line_near(l, r));
                        let e = Stop {
                            x: r.right(),
                            ..s.clone()
                        };
                        (s, e)
                    }
                };
                if i > 0 {
                    let offset = per[i - 1].len().saturating_sub(1);
                    let s = Stop {
                        cursor: cursor(block, at(i - 1), offset),
                        depth: 0,
                        ..first
                    };
                    if let Some(slot) = per[i - 1].last_mut() {
                        slot.get_or_insert(s);
                    }
                }
                if let Some(slot) = per.get_mut(i + 1).and_then(|v| v.first_mut()) {
                    let s = Stop {
                        cursor: cursor(block, at(i + 1), 0),
                        depth: 0,
                        ..last
                    };
                    slot.get_or_insert(s);
                }
            }
            Inline::Text(_) | Inline::Raw(_) => {
                let slots = &mut per[i];
                let glyphs = l.glyphs.iter().filter(|g| g.node == inline.id);
                // A position takes the glyph that starts there (so a wrapped
                // line's first char wins over the previous line's end).
                let mut ends = vec![];
                for g in glyphs {
                    let n = g.chars.len().max(1);
                    for k in 0..n {
                        let x = g.rect.x + g.rect.w * k as f64 / n as f64;
                        let s = stop(
                            block,
                            path.clone(),
                            g.chars.start + k,
                            x,
                            g.rect.y,
                            g.rect.bottom(),
                            g.baseline,
                        );
                        if let Some(slot) = slots.get_mut(g.chars.start + k) {
                            slot.get_or_insert(s);
                        }
                    }
                    let s = stop(
                        block,
                        path.clone(),
                        g.chars.end,
                        g.rect.right(),
                        g.rect.y,
                        g.rect.bottom(),
                        g.baseline,
                    );
                    ends.push(s);
                }
                for s in ends {
                    if let Some(slot) = slots.get_mut(s.cursor.offset) {
                        slot.get_or_insert(s);
                    }
                }
                // Inline code: the texts around it end and start at its box.
                if let (Inline::Raw(_), Some(r)) = (&inline.kind, l.box_of(inline.id)) {
                    let line = line_near(l, r);
                    if i > 0 {
                        let offset = per[i - 1].len().saturating_sub(1);
                        let s = stop(block, at(i - 1), offset, r.x, r.y, r.bottom(), line);
                        if let Some(slot) = per[i - 1].last_mut() {
                            slot.get_or_insert(s);
                        }
                    }
                    if let Some(slot) = per.get_mut(i + 1).and_then(|v| v.first_mut()) {
                        let s = stop(block, at(i + 1), 0, r.right(), r.y, r.bottom(), line);
                        slot.get_or_insert(s);
                    }
                }
            }
            Inline::Math(row) => {
                let Some(r) = l.box_of(inline.id) else {
                    continue;
                };
                let line = line_near(l, r);
                row_stops(block, path.clone(), row, l, line, 1, &mut inner[i]);
                if i > 0 {
                    let offset = per[i - 1].len().saturating_sub(1);
                    let s = stop(block, at(i - 1), offset, r.x, r.y, r.bottom(), line);
                    if let Some(slot) = per[i - 1].last_mut() {
                        slot.get_or_insert(s);
                    }
                }
                if let Some(slot) = per.get_mut(i + 1).and_then(|v| v.first_mut()) {
                    let s = stop(block, at(i + 1), 0, r.right(), r.y, r.bottom(), line);
                    slot.get_or_insert(s);
                }
            }
        }
    }

    // Fill positions without a glyph from their neighbours.
    let mut out = vec![];
    for (slots, inner) in per.into_iter().zip(inner) {
        out.extend(inner);
        let mut filled: Vec<Option<Stop>> = slots.clone();
        for k in 1..filled.len() {
            if filled[k].is_none()
                && let Some(prev) = filled[k - 1].clone()
            {
                filled[k] = Some(Stop {
                    cursor: CursorPath {
                        offset: k,
                        ..prev.cursor.clone()
                    },
                    x: prev.x + SPACE_PT,
                    ..prev
                });
            }
        }
        for k in (0..filled.len().saturating_sub(1)).rev() {
            if filled[k].is_none()
                && let Some(next) = filled[k + 1].clone()
            {
                filled[k] = Some(Stop {
                    cursor: CursorPath {
                        offset: k,
                        ..next.cursor.clone()
                    },
                    x: next.x - SPACE_PT,
                    ..next
                });
            }
        }
        out.extend(filled.into_iter().flatten());
    }
    out
}

/// The baseline of the text line a box sits on.
fn line_near(l: &BlockLayout, r: Rect) -> f64 {
    let cy = r.y + r.h / 2.0;
    l.glyphs
        .iter()
        .filter(|g| g.rect.y <= cy && cy <= g.rect.bottom())
        .min_by(|a, b| {
            let da = (a.rect.x - r.x)
                .abs()
                .min((a.rect.right() - r.right()).abs());
            let db = (b.rect.x - r.x)
                .abs()
                .min((b.rect.right() - r.right()).abs());
            da.total_cmp(&db)
        })
        .map_or(
            if l.glyphs.is_empty() {
                l.baseline
            } else {
                r.bottom()
            },
            |g| g.baseline,
        )
}

/// A mark over the formula being edited, where the render shows nothing:
/// an alignment point (`&`, a thin line between its neighbours) or a line
/// break (`\\`, after the end of its line). Drawn by the widget: in the
/// render a mark would change the formula's spacing (measured).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Align { x: f64, top: f64, bottom: f64 },
    Break { x: f64, top: f64, bottom: f64 },
}

/// The marks of the formula the caret is in (a display formula, or the
/// inline formula holding it), from the stops of its block; none when the
/// caret is not in a formula.
pub fn formula_marks(doc: &Document, caret: &CursorPath, stops: &[Stop]) -> Vec<Mark> {
    let Some(block) = doc.block(caret.block) else {
        return vec![];
    };
    let is_row = |path: &[Step]| matches!(doc.walk(block, path), Ok(Target::Row { .. }));
    // The path to the formula: none for display math, else through the
    // inline calls to the inline formula.
    let formula = match doc.inline_at(block, &caret.path) {
        _ if is_row(&[]) => &[][..],
        Some(at) if is_row(&caret.path[..at.prefix + 1]) => &caret.path[..at.prefix + 1],
        _ => return vec![],
    };
    let mut out = vec![];
    for s in stops {
        let c = &s.cursor;
        if c.block != caret.block || !c.path.starts_with(formula) {
            continue;
        }
        let Ok(Target::Row { row }) = doc.walk(block, &c.path) else {
            continue;
        };
        match row.nodes.get(c.offset).map(|n| &n.kind) {
            Some(MathKind::Align) => {
                let next = stops
                    .iter()
                    .find(|t| t.cursor.path == c.path && t.cursor.offset == c.offset + 1)
                    .map_or(s.x, |t| t.x);
                out.push(Mark::Align {
                    x: (s.x + next) / 2.0,
                    top: s.top,
                    bottom: s.bottom,
                });
            }
            Some(MathKind::Linebreak) => out.push(Mark::Break {
                x: s.x,
                top: s.top,
                bottom: s.bottom,
            }),
            _ => {}
        }
    }
    out
}

/// The outline of the inline call at `call` (a path) in `block`: one box
/// per line its text spans, from the stops inside it (top, bottom and the
/// extent of each line).
pub fn call_outline(stops: &[Stop], block: model::NodeId, call: &[Step]) -> Vec<Rect> {
    let mut body = call.to_vec();
    body.push(Step::Body);
    let inside: Vec<Stop> = stops
        .iter()
        .filter(|s| s.cursor.block == block && s.cursor.path.starts_with(&body))
        .cloned()
        .collect();
    lines(&inside)
        .into_iter()
        .map(|line| {
            let (mut x0, mut x1) = (f64::MAX, f64::MIN);
            let (mut top, mut bottom) = (f64::MAX, f64::MIN);
            for &i in &line {
                let s = &inside[i];
                x0 = x0.min(s.x);
                x1 = x1.max(s.x);
                top = top.min(s.top);
                bottom = bottom.max(s.bottom);
            }
            Rect {
                x: x0,
                y: top,
                w: x1 - x0,
                h: bottom - top,
            }
        })
        .collect()
}

/// The stop for a caret; if it is not among the stops (layout not updated
/// yet), the nearest earlier position in the same container.
pub fn find<'a>(stops: &'a [Stop], cursor: &CursorPath) -> Option<&'a Stop> {
    stops.iter().find(|s| s.cursor == *cursor).or_else(|| {
        stops
            .iter()
            .filter(|s| s.cursor.path == cursor.path && s.cursor.offset <= cursor.offset)
            .max_by_key(|s| s.cursor.offset)
    })
}

/// Lines of a block: indices into `stops`, grouped by baseline, top to bottom.
pub fn lines(stops: &[Stop]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..stops.len()).collect();
    order.sort_by(|&a, &b| stops[a].line.total_cmp(&stops[b].line));
    let mut out: Vec<Vec<usize>> = vec![];
    for i in order {
        match out.last_mut() {
            Some(line) if (stops[line[0]].line - stops[i].line).abs() < LINE_TOLERANCE_PT => {
                line.push(i)
            }
            _ => out.push(vec![i]),
        }
    }
    out
}

/// Penalty in pt per nesting level, so ties go to the outer position: for
/// line moves (prefer staying out of formulas) and for clicks (only ties).
const DEPTH_PT: f64 = 0.5;
const CLICK_DEPTH_PT: f64 = 0.01;

/// The stop of `line` closest to `x` (preferring shallower positions).
pub fn nearest_in_line<'a>(stops: &'a [Stop], line: &[usize], x: f64) -> Option<&'a Stop> {
    let dist = |s: &Stop| (s.x - x).abs() + DEPTH_PT * s.depth as f64;
    line.iter()
        .map(|&i| &stops[i])
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))
}

/// The first or last stop of a line.
pub fn line_edge<'a>(stops: &'a [Stop], line: &[usize], end: bool) -> Option<&'a Stop> {
    // Home/End stay inside an environment: its edges only on a line of
    // nothing else.
    let inside = line.iter().any(|&i| !stops[i].edge);
    let stops = line
        .iter()
        .map(|&i| &stops[i])
        .filter(|s| !(inside && s.edge));
    if end {
        stops.max_by(|a, b| a.x.total_cmp(&b.x))
    } else {
        stops.min_by(|a, b| a.x.total_cmp(&b.x))
    }
}

/// The caret position for a click at `(x, y)` in block coordinates: the
/// nearest line vertically, then the nearest position on it.
pub fn hit(stops: &[Stop], x: f64, y: f64) -> Option<&Stop> {
    let lines = lines(stops);
    let line = lines.iter().min_by(|a, b| {
        let key = |line: &Vec<usize>| {
            let top = line
                .iter()
                .map(|&i| stops[i].top)
                .fold(f64::INFINITY, f64::min);
            let bottom = line
                .iter()
                .map(|&i| stops[i].bottom)
                .fold(f64::NEG_INFINITY, f64::max);
            let outside = (top - y).max(y - bottom).max(0.0);
            (outside, (stops[line[0]].line - y).abs())
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.total_cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
    })?;
    // In a line, nested rows (numerator over denominator) share x ranges, so
    // the vertical distance to each position's own row counts too.
    let dist = |s: &Stop| {
        let dy = (s.top - y).max(y - s.bottom).max(0.0);
        (s.x - x).abs() + 2.0 * dy + CLICK_DEPTH_PT * s.depth as f64
    };
    line.iter()
        .map(|&i| &stops[i])
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))
}

/// The environment whose heading ("Theorem 2.1.", drawn by its function,
/// with no caret position of its own) is at `(x, y)`: on the line of its
/// first position inside, between its outline and that position.
pub fn heading_at(stops: &[Stop], x: f64, y: f64) -> Option<model::NodeId> {
    stops.windows(2).find_map(|w| {
        let (before, first) = (&w[0], &w[1]);
        let on_line = first.top - LINE_TOLERANCE_PT <= y && y <= first.bottom + LINE_TOLERANCE_PT;
        let heading = before.edge && before.cursor.offset == 0 && x > before.x && x < first.x - 1.0;
        (heading && on_line).then_some(before.cursor.block)
    })
}

/// The stop for `path` nearest to `x` (↑/↓ into a given row).
pub fn nearest_in_row<'a>(stops: &'a [Stop], path: &[Step], x: f64) -> Option<&'a Stop> {
    stops
        .iter()
        .filter(|s| s.cursor.path == path)
        .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{Document, ListKind};
    use typst_bridge::{Around, BridgeWorld, block_document, compile, raw_layout, serialize_block};

    fn layout_of(block: &BlockNode, width: f64) -> BlockLayout {
        let mut world = BridgeWorld::new();
        let (snippet, map) = serialize_block(block);
        let (src, offset) = block_document(&Around::default(), &snippet, width);
        let doc = compile(&mut world, &src).unwrap();
        raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
    }

    /// A call being typed is set in a code font: every char still has its
    /// stop, evenly spaced (monospace), from the start of the line.
    #[test]
    fn stops_in_a_call_being_typed() {
        let mut d = Document::empty();
        let t = d.text("#align(c");
        let p = d.paragraph(vec![t]);
        let s = stops(&p, Some(&layout_of(&p, 300.0)));
        assert_eq!(s.len(), 9);
        let steps: Vec<f64> = s.windows(2).map(|w| w[1].x - w[0].x).collect();
        assert!(s[0].x.abs() < 1.0, "{}", s[0].x);
        assert!(
            steps.iter().all(|d| (d - steps[0]).abs() < 0.1 && *d > 3.0),
            "{steps:?}"
        );
    }

    /// A two-line aligned formula: the stops of each line have that line's
    /// height and baseline; the second line starts at its own left, not at
    /// the end of the first.
    #[test]
    fn stops_in_multi_line_formulas() {
        let mut d = Document::empty();
        let (a, b, c) = (d.ident("a"), d.ident("b"), d.ident("c"));
        let (eq1, eq2) = (d.sym("="), d.sym("="));
        let (al1, al2) = (d.node(MathKind::Align), d.node(MathKind::Align));
        let br = d.node(MathKind::Linebreak);
        let block = d.display(vec![a, al1, eq1, b, br, al2, eq2, c]);
        let l = layout_of(&block, 300.0);
        let s = stops(&block, Some(&l));
        assert_eq!(s.len(), 9);
        let (first, second) = (&s[..5], &s[5..]);
        let line1 = first[0].line;
        assert!(first.iter().all(|t| t.line == line1), "{first:?}");
        assert!(second.iter().all(|t| t.line > line1 + 5.0), "{second:?}");
        assert!(second[0].top >= first[0].bottom - 1.0, "own height");
        assert!(
            second[0].x < first[4].x,
            "line 2 starts left of line 1's end"
        );
        // Aligned at `&`: the `=` of both lines line up.
        assert!(
            (first[2].x - second[1].x).abs() < 0.5,
            "{first:?} {second:?}"
        );
    }

    /// A break at the end of a formula opens an empty line, which shows a
    /// placeholder; the caret after the break sits in it, below the first
    /// line.
    #[test]
    fn caret_on_an_empty_new_line() {
        let mut d = Document::empty();
        let a = d.ident("a");
        let br = d.node(MathKind::Linebreak);
        let block = d.display(vec![a, br]);
        let l = layout_of(&block, 300.0);
        let s = stops(&block, Some(&l));
        assert_eq!(s.len(), 3);
        let slot = l.box_of(block.rows()[0].nodes[1].id).unwrap();
        assert!(s[1].line == s[0].line, "{s:?}");
        assert!(s[2].line > s[0].line + 5.0, "{s:?}");
        assert!(s[2].top >= s[0].bottom - 1.0, "{s:?}");
        assert!(
            (s[2].x - (slot.x + slot.w / 2.0)).abs() < 0.01,
            "{s:?} {slot:?}"
        );
    }

    /// Marks show for the formula the caret is in: `&` between its
    /// neighbours, `\\` at the end of its line; none with the caret in
    /// text.
    #[test]
    fn marks_of_the_formula_being_edited() {
        let mut d = Document::empty();
        let (a, b, c) = (d.ident("a"), d.ident("b"), d.ident("c"));
        let eq = d.sym("=");
        let al = d.node(MathKind::Align);
        let br = d.node(MathKind::Linebreak);
        let block = d.display(vec![a, al, eq, b, br, c]);
        let id = block.id;
        let l = layout_of(&block, 300.0);
        let s = stops(&block, Some(&l));
        d.blocks = vec![block];
        let caret = CursorPath {
            block: id,
            path: vec![],
            offset: 0,
        };
        let marks = formula_marks(&d, &caret, &s);
        let [Mark::Align { x: ax, .. }, Mark::Break { x: bx, .. }] = marks[..] else {
            panic!("{marks:?}");
        };
        assert!(s[1].x < ax && ax < s[2].x, "{ax} {s:?}");
        assert_eq!(bx, s[4].x);
        let mut p = Document::empty();
        let text = p.paragraph(vec![]);
        let caret = CursorPath {
            block: text.id,
            path: vec![Step::Index(0)],
            offset: 0,
        };
        p.blocks = vec![text];
        assert!(formula_marks(&p, &caret, &s).is_empty());
    }

    /// A numbered display formula: the number belongs to no node, so the
    /// caret ends right after the formula (also with one node), and in an
    /// empty one sits in the middle of the placeholder.
    #[test]
    fn numbered_display_formulas() {
        let mut world = BridgeWorld::new();
        let around = Around {
            header: "#set math.equation(numbering: \"(1)\")\n".into(),
            ..Around::default()
        };
        let mut d = Document::empty();
        let x = d.ident("x");
        for (block, empty) in [(d.display(vec![x]), false), (d.display(vec![]), true)] {
            let (snippet, map) = serialize_block(&block);
            let (src, offset) = block_document(&around, &snippet, 300.0);
            let doc = compile(&mut world, &src).unwrap();
            let l = raw_layout(&world, &doc.pages()[0], offset).resolve(&map);
            let rb = l.box_of(block.rows()[0].id).unwrap();
            assert!(
                rb.right() < 200.0,
                "the row stops before the number: {rb:?}"
            );
            let end = stops(&block, Some(&l)).last().unwrap().x;
            let want = if empty { rb.x + rb.w / 2.0 } else { rb.right() };
            assert!((end - want).abs() < 0.01, "{end} vs {rb:?}");
        }
    }

    /// In a numbered heading without text the caret sits after the number,
    /// where typed text appears.
    #[test]
    fn empty_numbered_heading_puts_the_caret_after_the_number() {
        let mut d = Document::empty();
        let empty = d.heading(1, vec![]);
        let t = d.text("A");
        let full = d.heading(1, vec![t]);
        let layout = |block: &BlockNode| {
            let mut world = BridgeWorld::new();
            let (snippet, map) = serialize_block(block);
            let around = Around {
                header: typst_bridge::DEFAULT_HEADER.into(),
                ..Around::default()
            };
            let (src, offset) = block_document(&around, &snippet, 300.0);
            let doc = compile(&mut world, &src).unwrap();
            raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
        };
        let s = stops(&empty, Some(&layout(&empty)));
        assert_eq!(s.len(), 1);
        let typed = stops(&full, Some(&layout(&full)))[0].x;
        assert!(s[0].x > 5.0, "after the number: {}", s[0].x);
        assert!((s[0].x - typed).abs() < 2.5, "{} vs {typed}", s[0].x);
    }

    /// Stops inside an environment come from its one render: after the
    /// environment's label in a fresh one, and for every char of its
    /// paragraphs, the second below the first.
    #[test]
    fn stops_inside_environments() {
        let header = "#let lemma(body) = block(width: 100%)[*Lemma 1.* #emph(body)]";
        let layout = |block: &BlockNode| {
            let mut world = BridgeWorld::new();
            let (snippet, map) = serialize_block(block);
            let around = Around {
                header: header.into(),
                ..Around::default()
            };
            let (src, offset) = block_document(&around, &snippet, 300.0);
            let doc = compile(&mut world, &src).unwrap();
            raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
        };

        let mut d = Document::empty();
        let fresh = d.env("lemma", vec![]);
        let fresh_layout = layout(&fresh);
        let s = stops(&fresh, Some(&fresh_layout));
        // Just before it, inside it, just after it.
        let offsets: Vec<_> = s
            .iter()
            .map(|s| (s.cursor.block, s.cursor.offset))
            .collect();
        let body = fresh.kind.body().unwrap()[0].id;
        assert_eq!(offsets, [(fresh.id, 0), (body, 0), (fresh.id, 1)]);
        assert!(
            s[0].x < 0.0 && s[2].x > fresh_layout.width,
            "outside its box"
        );
        // Home/End on that line stay inside.
        let line: Vec<usize> = (0..s.len()).collect();
        assert_eq!(line_edge(&s, &line, true).unwrap().cursor.block, body);
        assert_eq!(line_edge(&s, &line, false).unwrap().cursor.block, body);
        let one = d.text("A");
        let p = d.paragraph(vec![one]);
        let typed = d.env("lemma", vec![p]);
        let typed_x = stops(&typed, Some(&layout(&typed)))[1].x;
        assert!(s[1].x > 30.0, "after the label: {}", s[1].x);
        assert!((s[1].x - typed_x).abs() < 1.0, "{} vs {typed_x}", s[1].x);
        // A click on "Lemma 1." is on its heading; after it, not.
        let mid = (s[1].top + s[1].bottom) / 2.0;
        assert_eq!(heading_at(&s, 10.0, mid), Some(fresh.id));
        assert_eq!(heading_at(&s, s[1].x + 5.0, mid), None);
        assert_eq!(heading_at(&s, 10.0, s[1].bottom + 20.0), None);

        let (first, second) = (d.text("First"), d.text("Second one"));
        let (p1, p2) = (d.paragraph(vec![first]), d.paragraph(vec![second]));
        let (id1, id2) = (p1.id, p2.id);
        let env = d.env("lemma", vec![p1, p2]);
        let s = stops(&env, Some(&layout(&env)));
        let of = |id| {
            s.iter()
                .filter(|s| s.cursor.block == id)
                .collect::<Vec<_>>()
        };
        assert_eq!(of(id1).len(), "First".len() + 1);
        assert_eq!(of(id2).len(), "Second one".len() + 1);
        assert!(
            of(id2)[0].top > of(id1)[0].bottom - 0.1,
            "second paragraph below"
        );
    }

    /// A title's stops come between "just before" and the body, inside the
    /// parentheses; an empty title (being typed) has one.
    #[test]
    fn stops_in_environment_titles() {
        let header = "#let lemma(..args) = {
  let (title, body) = if args.pos().len() > 1 { args.pos() } else { (none, ..args.pos()) }
  block(width: 100%)[*Lemma 1#if title != none [ (#title)].* #emph(body)]
}";
        let layout = |block: &BlockNode| {
            let mut world = BridgeWorld::new();
            let (snippet, map) = serialize_block(block);
            let around = Around {
                header: header.into(),
                ..Around::default()
            };
            let (src, offset) = block_document(&around, &snippet, 300.0);
            let doc = compile(&mut world, &src).unwrap();
            raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
        };
        let mut d = Document::empty();
        for title in ["", "Key"] {
            let text = d.text("Text");
            let p = d.paragraph(vec![text]);
            let body = p.id;
            let mut env = d.env("lemma", vec![p]);
            let t = d.text(title);
            if let Block::Env { title, .. } = &mut env.kind {
                title.push(t);
            }
            let s = stops(&env, Some(&layout(&env)));
            let in_title: Vec<&Stop> = s
                .iter()
                .filter(|s| s.cursor.block == env.id && !s.cursor.path.is_empty())
                .collect();
            assert_eq!(in_title.len(), title.len() + 1, "title {title:?}");
            let body_x = s.iter().find(|s| s.cursor.block == body).unwrap().x;
            assert!(s[0].edge && s[1] == *in_title[0]);
            assert!(
                30.0 < in_title[0].x && in_title.last().unwrap().x + 5.0 < body_x,
                "title {title:?}: {} .. {}, body at {body_x}",
                in_title[0].x,
                in_title.last().unwrap().x
            );
        }
    }

    /// A list item's text has its stops after the marker; a nested item's
    /// further in, on the next line; an empty item has one.
    #[test]
    fn stops_in_list_items() {
        let layout = |block: &BlockNode| {
            let mut world = BridgeWorld::new();
            let (snippet, map) = serialize_block(block);
            let (src, offset) = block_document(&Around::default(), &snippet, 300.0);
            let doc = compile(&mut world, &src).unwrap();
            raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
        };
        let mut d = Document::empty();
        let (a, b) = (d.text("Outer"), d.text("Inner"));
        let (pa, pb) = (d.paragraph(vec![a]), d.paragraph(vec![b]));
        let (ida, idb) = (pa.id, pb.id);
        let inner = d.item(ListKind::Bullet, vec![pb]);
        let outer = d.item(ListKind::Bullet, vec![pa, inner]);
        let s = stops(&outer, Some(&layout(&outer)));
        let first = |id| s.iter().find(|s| s.cursor.block == id).unwrap();
        assert_eq!(s.len(), "Outer".len() + "Inner".len() + 2);
        assert!(first(ida).x > 5.0, "after the bullet: {}", first(ida).x);
        assert!(first(idb).x > first(ida).x + 5.0);
        assert!(first(idb).top > first(ida).bottom - 0.1);

        let empty = d.item(ListKind::Bullet, vec![]);
        let s = stops(&empty, Some(&layout(&empty)));
        assert_eq!(s.len(), 1);
        assert!(s[0].x > 5.0, "after the bullet: {}", s[0].x);
    }

    /// The acceptance check of the prose milestone: clicking anywhere on a
    /// glyph puts the caret at one of its edges, the nearer one.
    #[test]
    fn click_lands_on_the_clicked_glyph() {
        let mut d = Document::empty();
        let t1 = d.text("Office fluffy text, wrapped over several lines of a narrow page: ");
        let x = d.ident("x");
        let m = d.math(vec![x]);
        let t2 = d.text(" and more words after the formula to wrap again.");
        let block = d.paragraph(vec![t1, m, t2]);
        let l = layout_of(&block, 150.0);
        let s = stops(&block, Some(&l));
        assert!(lines(&s).len() >= 3, "text wraps");
        for g in &l.glyphs {
            let y = g.rect.y + g.rect.h / 2.0;
            let n = g.chars.len();
            let line_end = !l
                .glyphs
                .iter()
                .any(|o| (o.baseline - g.baseline).abs() < 1.0 && o.rect.x > g.rect.x);
            for k in 0..n {
                // Ligatures: aim within the char's own share of the glyph.
                for (fx, want) in [(0.2, g.chars.start + k), (0.8, g.chars.start + k + 1)] {
                    let x = g.rect.x + g.rect.w * (k as f64 + fx) / n as f64;
                    let hit = hit(&s, x, y).unwrap();
                    assert_eq!(hit.cursor.path, vec![Step::Index(index_of(&block, g.node))]);
                    let got = hit.cursor.offset;
                    // After the last glyph of a wrapped line, the position
                    // may belong to the next line; then we land before it.
                    let ok = got == want || (line_end && got + 1 == want);
                    assert!(ok, "glyph {:?} at {fx}: got {got}, want {want}", g.chars);
                    assert!((hit.x - x).abs() <= g.rect.w, "caret within one glyph");
                }
            }
        }
    }

    fn index_of(block: &BlockNode, id: model::NodeId) -> usize {
        block
            .kind
            .inlines()
            .unwrap()
            .iter()
            .position(|i| i.id == id)
            .unwrap()
    }

    #[test]
    fn every_text_position_has_a_stop() {
        let mut d = Document::empty();
        let t1 = d.text("a  b ");
        let x = d.ident("x");
        let m = d.math(vec![x]);
        let t2 = d.text("");
        let block = d.paragraph(vec![t1, m, t2]);
        let l = layout_of(&block, 300.0);
        let s = stops(&block, Some(&l));
        let offsets: Vec<_> = s
            .iter()
            .map(|s| (s.cursor.path.clone(), s.cursor.offset))
            .collect();
        let want: Vec<_> = (0..=5)
            .map(|k| (vec![Step::Index(0)], k))
            .chain([(vec![Step::Index(1)], 0), (vec![Step::Index(1)], 1)])
            .chain([(vec![Step::Index(2)], 0)])
            .collect();
        assert_eq!(offsets, want);
        let xs: Vec<_> = s.iter().filter(|s| s.depth == 0).map(|s| s.x).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1] + 0.01), "{xs:?}");
    }

    /// Every position of the document's model has a stop, in reading order
    /// (left to right on one line), inside bold, italic and calls too; the
    /// positions just before and after a call sit where its text starts and
    /// ends.
    #[test]
    fn stops_in_inline_calls() {
        for keys in [
            "a `code` c",
            "`x y` and `` and `a`",
            "a\\ b\\ ",
            "a~b -- c",
            "a *bold* c",
            "*x _y $a$ z_*.",
            "see #underline[it] now",
            "x *",
            "*a*",
        ] {
            let mut e = model::Editor::new(Document::new());
            e.insert_text(keys);
            let block = e.doc().blocks[0].clone();
            let l = layout_of(&block, 300.0);
            let s = stops(&block, Some(&l));
            let got: Vec<_> = s.iter().map(|s| s.cursor.clone()).collect();
            assert_eq!(got, e.doc().positions(&block), "{keys}");
            // Left to right on a line; a line break starts a new line.
            let xs: Vec<_> = s
                .iter()
                .filter(|s| s.depth == 0)
                .map(|s| (s.x, s.line))
                .collect();
            assert!(
                xs.windows(2)
                    .all(|w| w[0].0 <= w[1].0 + 0.01 || w[1].1 > w[0].1 + 2.0),
                "{keys}: {xs:?}"
            );
        }
        // Every char of inline code has its own place (monospace: even steps).
        let mut e = model::Editor::new(Document::new());
        e.insert_text("a `code` c");
        let block = e.doc().blocks[0].clone();
        let s = stops(&block, Some(&layout_of(&block, 300.0)));
        let xs: Vec<f64> = s
            .iter()
            .filter(|s| s.cursor.path == [Step::Index(1)])
            .map(|s| s.x)
            .collect();
        let steps: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(xs.len(), 5);
        assert!(
            steps.iter().all(|d| (d - steps[0]).abs() < 0.1 && *d > 3.0),
            "{xs:?}"
        );
        // After a line break the caret is at the start of the next line, on
        // an empty one too.
        let mut e = model::Editor::new(Document::new());
        e.insert_text("ab\\ ");
        let block = e.doc().blocks[0].clone();
        let s = stops(&block, Some(&layout_of(&block, 300.0)));
        assert_eq!(s.len(), 4);
        assert!(s[3].x.abs() < 0.5 && s[3].line > s[2].line + 5.0, "{s:?}");
    }

    /// The outline of a call covers its text, one box per line.
    #[test]
    fn outline_of_an_inline_call() {
        let mut e = model::Editor::new(Document::new());
        e.insert_text("a *bold* c");
        let block = e.doc().blocks[0].clone();
        let l = layout_of(&block, 300.0);
        let s = stops(&block, Some(&l));
        let call = [Step::Index(1)];
        let boxes = call_outline(&s, block.id, &call);
        assert_eq!(boxes.len(), 1);
        let bold = l.box_of(block.kind.inlines().unwrap()[1].id).unwrap();
        let b = boxes[0];
        assert!(
            (b.x - bold.x).abs() < 0.5 && (b.right() - bold.right()).abs() < 0.5,
            "{b:?} {bold:?}"
        );
        // Wrapped over lines: a box per line.
        let mut e = model::Editor::new(Document::new());
        e.insert_text("a *bold words that go on and on over the end of a narrow line* c");
        let block = e.doc().blocks[0].clone();
        let l = layout_of(&block, 120.0);
        let s = stops(&block, Some(&l));
        assert!(call_outline(&s, block.id, &call).len() > 1);
    }

    #[test]
    fn clicks_reach_numerator_and_denominator() {
        let mut d = Document::empty();
        let (a, b, c) = (d.ident("a"), d.ident("b"), d.ident("c"));
        let f = d.frac(vec![a, b], vec![c]);
        let disp = d.display(vec![f]);
        let l = layout_of(&disp, 300.0);
        let s = stops(&disp, Some(&l));
        let model::Block::DisplayMath(row) = &disp.kind else {
            unreachable!()
        };
        let model::MathKind::Frac { num, den } = &row.nodes[0].kind else {
            unreachable!()
        };
        for rowid in [num.id, den.id] {
            let r = l.box_of(rowid).unwrap();
            let hit = hit(&s, r.x + r.w * 0.3, r.y + r.h / 2.0).unwrap();
            let want = if rowid == num.id {
                Step::Num
            } else {
                Step::Den
            };
            assert_eq!(hit.cursor.path, vec![Step::Index(0), want]);
        }
        // Every position of the block has a stop.
        assert_eq!(s.len(), 2 + 3 + 2);
    }

    #[test]
    fn empty_paragraph_and_display_math() {
        let mut d = Document::empty();
        let p = d.paragraph(vec![]);
        let l = layout_of(&p, 300.0);
        let s = stops(&p, Some(&l));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].cursor.path, vec![Step::Index(0)]);
        assert_eq!(s[0].x, 0.0);

        let (a, b) = (d.ident("a"), d.ident("b"));
        let disp = d.display(vec![a, b]);
        let l = layout_of(&disp, 300.0);
        let s = stops(&disp, Some(&l));
        let offsets: Vec<_> = s.iter().map(|s| s.cursor.offset).collect();
        assert_eq!(offsets, [0, 1, 2]);
        assert!(s[0].x > 100.0, "centred");
        let all: Vec<usize> = (0..s.len()).collect();
        assert_eq!(line_edge(&s, &all, false).unwrap().cursor.offset, 0);
        assert_eq!(line_edge(&s, &all, true).unwrap().cursor.offset, 2);
    }
}
