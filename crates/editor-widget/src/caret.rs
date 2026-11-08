//! Caret geometry: every caret position of a block with its place on the
//! rendered page. Built from the block's [`BlockLayout`]; no GTK here.
//!
//! Text positions come from glyph clusters (a ligature's width is split
//! evenly between its chars). Positions without a glyph (collapsed or
//! trailing spaces) are placed after the previous one. Math rows put a
//! position at the left edge of each node and after the last one, and the
//! middle of an empty slot; positions in a row span the row's height.

use model::{Block, BlockNode, CursorPath, Inline, MathRow, Step};
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
    let mut right = rb.x;
    for (k, node) in row.nodes.iter().enumerate() {
        let r = l.box_of(node.id);
        let x = r.map_or(right, |r| r.x);
        out.push(at(path.clone(), k, x));
        if let Some(r) = r {
            right = r.right();
        }
        for (step, child) in node.children() {
            let mut p = path.clone();
            p.extend([Step::Index(k), step]);
            row_stops(block, p, child, l, line, depth + 1, out);
        }
    }
    out.push(at(path, row.nodes.len(), right));
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
    // Per text inline: stop by char offset.
    let mut per: Vec<Vec<Option<Stop>>> = inlines
        .iter()
        .map(|i| match &i.kind {
            Inline::Text(s) => vec![None; s.chars().count() + 1],
            Inline::Math(_) => vec![],
        })
        .collect();

    // Per formula: the stops inside it.
    let mut inner: Vec<Vec<Stop>> = vec![vec![]; inlines.len()];

    for (i, inline) in inlines.iter().enumerate() {
        let path = vec![Step::Index(i)];
        match &inline.kind {
            Inline::Text(_) => {
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
            }
            Inline::Math(row) => {
                let Some(r) = l.box_of(inline.id) else {
                    continue;
                };
                let line = line_near(l, r);
                row_stops(block, path.clone(), row, l, line, 1, &mut inner[i]);
                if i > 0 {
                    let offset = per[i - 1].len().saturating_sub(1);
                    let s = stop(
                        block,
                        vec![Step::Index(i - 1)],
                        offset,
                        r.x,
                        r.y,
                        r.bottom(),
                        line,
                    );
                    if let Some(slot) = per[i - 1].last_mut() {
                        slot.get_or_insert(s);
                    }
                }
                if let Some(slot) = per.get_mut(i + 1).and_then(|v| v.first_mut()) {
                    let s = stop(
                        block,
                        vec![Step::Index(i + 1)],
                        0,
                        r.right(),
                        r.y,
                        r.bottom(),
                        line,
                    );
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
    use model::Document;
    use typst_bridge::{Around, BridgeWorld, block_document, compile, raw_layout, serialize_block};

    fn layout_of(block: &BlockNode, width: f64) -> BlockLayout {
        let mut world = BridgeWorld::new();
        let (snippet, map) = serialize_block(block);
        let (src, offset) = block_document(&Around::default(), &snippet, width);
        let doc = compile(&mut world, &src).unwrap();
        raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
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
