//! Caret geometry: every caret position of a block with its place on the
//! rendered page. Built from the block's [`BlockLayout`]; no GTK here.
//!
//! Text positions come from glyph clusters (a ligature's width is split
//! evenly between its chars). Positions without a glyph (collapsed or
//! trailing spaces) are placed after the previous one. Formulas inside a
//! paragraph contribute the positions just before and after them; display
//! formulas their top-level positions.

use model::{Block, BlockNode, CursorPath, Inline, Step};
use typst_bridge::{BlockLayout, Rect};

/// Advance used for chars that produced no glyph, in pt (≈ a space at 11 pt).
const SPACE_PT: f64 = 3.7;
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
}

/// All caret positions of a block, in document order.
pub fn stops(block: &BlockNode, layout: Option<&BlockLayout>) -> Vec<Stop> {
    let mut out = match (layout, &block.kind) {
        (None, _) => vec![],
        (Some(l), Block::Paragraph(inlines) | Block::Heading { inlines, .. }) => {
            inline_stops(block, inlines, l)
        }
        (Some(l), Block::DisplayMath(row)) => {
            let row_box = l.box_of(row.id);
            let (top, bottom) = row_box.map_or((0.0, l.height), |r| (r.y, r.bottom()));
            let mut v = vec![];
            for (k, node) in row.nodes.iter().enumerate() {
                if let Some(r) = l.box_of(node.id) {
                    v.push(stop(block, vec![], k, r.x, top, bottom, l.baseline));
                    if k + 1 == row.nodes.len() {
                        v.push(stop(
                            block,
                            vec![],
                            k + 1,
                            r.right(),
                            top,
                            bottom,
                            l.baseline,
                        ));
                    }
                }
            }
            if row.nodes.is_empty()
                && let Some(r) = row_box
            {
                v.push(stop(block, vec![], 0, r.x, top, bottom, l.baseline));
            }
            v
        }
        (Some(l), Block::RawTypst(text)) => {
            let n = text.chars().count();
            vec![
                stop(block, vec![], 0, 0.0, 0.0, l.height, l.baseline),
                stop(block, vec![], n, l.width, 0.0, l.height, l.baseline),
            ]
        }
    };
    if out.is_empty() {
        // Not rendered yet, or nothing visible (an empty paragraph).
        let start = match block.kind.inlines() {
            Some(_) => vec![Step::Index(0)],
            None => vec![],
        };
        let baseline = layout.map_or(EMPTY_BOTTOM_PT * 0.8, |l| l.baseline.max(1.0));
        out.push(Stop {
            cursor: CursorPath {
                block: block.id,
                path: start,
                offset: 0,
            },
            x: 0.0,
            top: EMPTY_TOP_PT,
            bottom: EMPTY_BOTTOM_PT,
            line: baseline,
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
            Inline::Math(_) => {
                let Some(r) = l.box_of(inline.id) else {
                    continue;
                };
                let line = line_near(l, r);
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
    for slots in per {
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

/// The stop of `line` closest to `x`.
pub fn nearest_in_line<'a>(stops: &'a [Stop], line: &[usize], x: f64) -> Option<&'a Stop> {
    line.iter()
        .map(|&i| &stops[i])
        .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
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
    nearest_in_line(stops, line, x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Document;
    use typst_bridge::{BridgeWorld, block_document, compile, raw_layout, serialize_block};

    fn layout_of(block: &BlockNode, width: f64) -> BlockLayout {
        let mut world = BridgeWorld::new();
        let (snippet, map) = serialize_block(block);
        let (src, offset) = block_document(&snippet, width);
        let doc = compile(&mut world, &src).unwrap();
        raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
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
            .chain([(vec![Step::Index(2)], 0)])
            .collect();
        assert_eq!(offsets, want);
        let xs: Vec<_> = s.iter().map(|s| s.x).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1] + 0.01), "{xs:?}");
    }

    #[test]
    fn empty_paragraph_and_display_math() {
        let mut d = Document::empty();
        let p = d.paragraph(vec![]);
        let l = layout_of(&p, 300.0);
        let s = stops(&p, Some(&l));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].cursor.path, vec![Step::Index(0)]);

        let (a, b) = (d.ident("a"), d.ident("b"));
        let disp = d.display(vec![a, b]);
        let l = layout_of(&disp, 300.0);
        let s = stops(&disp, Some(&l));
        let offsets: Vec<_> = s.iter().map(|s| s.cursor.offset).collect();
        assert_eq!(offsets, [0, 1, 2]);
        assert!(s[0].x > 100.0, "centred");
    }
}
