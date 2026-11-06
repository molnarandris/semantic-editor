//! Geometry of a compiled block, mapped back to model nodes.
//!
//! Every glyph and shape in the frame tree carries a [`Span`]; we resolve it to
//! a byte offset in the block document, and the block's [`SpanMap`] turns
//! that into node ids. A node's box is the union of all marks inside its
//! source range, so structures get boxes from their descendants.

use std::collections::HashMap;
use std::ops::Range;

use model::NodeId;
use typst::WorldExt;
use typst::layout::{Abs, Frame, FrameItem, Point, Transform};
use typst::syntax::Span;
use typst_layout::Page;

use crate::compile::BLOCK_PAD_PT;
use crate::serialize::SpanMap;
use crate::world::BridgeWorld;

/// An axis-aligned rectangle in points, relative to the block's top left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    fn from_corners(a: Point, b: Point) -> Rect {
        let (x0, x1) = (a.x.to_pt().min(b.x.to_pt()), a.x.to_pt().max(b.x.to_pt()));
        let (y0, y1) = (a.y.to_pt().min(b.y.to_pt()), a.y.to_pt().max(b.y.to_pt()));
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// One glyph cluster of a text inline: chars `chars` of node `node`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBox {
    pub node: NodeId,
    pub chars: Range<usize>,
    pub rect: Rect,
    /// Baseline y of the glyph's line.
    pub baseline: f64,
}

/// Geometry of one rendered block, in points, relative to the top left of the
/// block content. The raster extends [`BLOCK_PAD_PT`] beyond it on each side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockLayout {
    pub width: f64,
    pub height: f64,
    /// Baseline of the first line.
    pub baseline: f64,
    /// Box of every node that produced visible output, parents first.
    pub boxes: Vec<(NodeId, Rect)>,
    /// Glyph clusters of text inlines, in visual order per line.
    pub glyphs: Vec<GlyphBox>,
}

impl BlockLayout {
    pub fn box_of(&self, id: NodeId) -> Option<Rect> {
        self.boxes.iter().find(|(n, _)| *n == id).map(|(_, r)| *r)
    }

    /// The smallest node box containing the point.
    pub fn hit(&self, x: f64, y: f64) -> Option<NodeId> {
        self.boxes
            .iter()
            .rev()
            .filter(|(_, r)| r.contains(x, y))
            .min_by(|(_, a), (_, b)| (a.w * a.h).total_cmp(&(b.w * b.h)))
            .map(|(n, _)| *n)
    }
}

/// A visible item and the source byte it came from.
#[derive(Clone, Debug)]
struct Mark {
    offset: usize,
    rect: Rect,
}

struct Walker<'a> {
    world: &'a BridgeWorld,
    ranges: HashMap<Span, Option<Range<usize>>>,
    marks: Vec<Mark>,
    /// Text glyphs: (source offset, cluster char count, rect, baseline).
    text: Vec<(usize, usize, Rect, f64)>,
}

impl Walker<'_> {
    fn range(&mut self, span: Span) -> Option<Range<usize>> {
        let world = self.world;
        self.ranges
            .entry(span)
            .or_insert_with(|| world.range(span))
            .clone()
    }

    fn frame(&mut self, frame: &Frame, ts: Transform) {
        for (pos, item) in frame.items() {
            match item {
                FrameItem::Group(group) => {
                    let ts = ts
                        .pre_concat(Transform::translate(pos.x, pos.y))
                        .pre_concat(group.transform);
                    self.frame(&group.frame, ts);
                }
                FrameItem::Text(text) => {
                    let metrics = text.font.metrics();
                    // Vertical extent relative to the baseline (y down): the
                    // font's line metrics, grown to the ink so stretched
                    // delimiters and big operators get their real height.
                    let mut top = -metrics.ascender.at(text.size).to_pt();
                    let mut bottom = -metrics.descender.at(text.size).to_pt();
                    let ink = text.bbox();
                    let (y0, y1) = (ink.min.y.to_pt(), ink.max.y.to_pt());
                    if y0.is_finite() && y1.is_finite() {
                        top = top.min(y0.min(y1));
                        bottom = bottom.max(y0.max(y1));
                    }
                    let (top, bottom) = (Abs::pt(top), Abs::pt(bottom));
                    let mut x = pos.x;
                    for glyph in &text.glyphs {
                        let advance = glyph.x_advance.at(text.size);
                        let left = x + glyph.x_offset.at(text.size);
                        let a = Point::new(left, pos.y + top).transform(ts);
                        let b = Point::new(left + advance, pos.y + bottom).transform(ts);
                        let rect = Rect::from_corners(a, b);
                        x += advance;
                        let (span, delta) = glyph.span;
                        let Some(range) = self.range(span) else {
                            continue;
                        };
                        let offset = range.start + delta as usize;
                        let chars = text.text[glyph.range()].chars().count();
                        let baseline = pos.transform(ts).y.to_pt();
                        self.marks.push(Mark { offset, rect });
                        self.text.push((offset, chars, rect, baseline));
                    }
                }
                FrameItem::Shape(shape, span) => {
                    let Some(range) = self.range(*span) else {
                        continue;
                    };
                    let bbox = shape.geometry.bbox(shape.stroke.as_ref());
                    let a = (*pos + bbox.min).transform(ts);
                    // Keep hairlines (fraction bars) hit-testable.
                    let max = Point::new(bbox.max.x, bbox.max.y.max(bbox.min.y + Abs::pt(0.5)));
                    let b = (*pos + max).transform(ts);
                    self.marks.push(Mark {
                        offset: range.start,
                        rect: Rect::from_corners(a, b),
                    });
                }
                FrameItem::Image(_, size, span) => {
                    let Some(range) = self.range(*span) else {
                        continue;
                    };
                    let a = pos.transform(ts);
                    let b = Point::new(pos.x + size.x, pos.y + size.y).transform(ts);
                    self.marks.push(Mark {
                        offset: range.start,
                        rect: Rect::from_corners(a, b),
                    });
                }
                FrameItem::Link(..) | FrameItem::Tag(_) => {}
            }
        }
    }
}

/// Geometry of a compiled block before mapping to nodes: marks are keyed by
/// byte offset in the snippet. Independent of node ids, so it can be cached
/// by source text and resolved against any block's span map.
#[derive(Clone, Debug, Default)]
pub struct RawLayout {
    width: f64,
    height: f64,
    baseline: f64,
    marks: Vec<Mark>,
    /// Text glyphs: (snippet offset, cluster char count, rect, baseline).
    text: Vec<(usize, usize, Rect, f64)>,
}

/// Extract the raw geometry of a compiled block page. `snippet_offset` is the
/// byte offset of the snippet in the compiled source.
pub fn raw_layout(world: &BridgeWorld, page: &Page, snippet_offset: usize) -> RawLayout {
    let frame = &page.frame;
    let mut walker = Walker {
        world,
        ranges: HashMap::new(),
        marks: vec![],
        text: vec![],
    };
    // Content coordinates: the page margin is the top-left origin.
    let pad = Abs::pt(BLOCK_PAD_PT);
    walker.frame(frame, Transform::translate(-pad, -pad));
    let local = |offset: usize| offset.checked_sub(snippet_offset);
    RawLayout {
        width: frame.width().to_pt() - 2.0 * BLOCK_PAD_PT,
        height: frame.height().to_pt() - 2.0 * BLOCK_PAD_PT,
        baseline: frame.baseline().to_pt() - BLOCK_PAD_PT,
        marks: walker
            .marks
            .into_iter()
            .filter_map(|m| {
                Some(Mark {
                    offset: local(m.offset)?,
                    rect: m.rect,
                })
            })
            .collect(),
        text: walker
            .text
            .into_iter()
            .filter_map(|(o, n, r, b)| Some((local(o)?, n, r, b)))
            .collect(),
    }
}

impl RawLayout {
    /// Map the geometry to nodes through the block's span map.
    pub fn resolve(&self, map: &SpanMap) -> BlockLayout {
        let mut boxes = vec![];
        for (id, range) in map.entries() {
            let rect = self
                .marks
                .iter()
                .filter(|m| range.contains(&m.offset))
                .map(|m| m.rect)
                .reduce(|a, b| a.union(&b));
            if let Some(rect) = rect {
                boxes.push((*id, rect));
            }
        }
        let mut glyphs = vec![];
        for &(offset, count, rect, baseline) in &self.text {
            let Some(node) = map.innermost(offset) else {
                continue;
            };
            let Some(start) = map.text_char_at(node, offset) else {
                continue;
            };
            glyphs.push(GlyphBox {
                node,
                chars: start..start + count,
                rect,
                baseline,
            });
        }
        BlockLayout {
            width: self.width,
            height: self.height,
            baseline: self.baseline,
            boxes,
            glyphs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{block_document, compile, serialize_block};
    use model::{Document, MathKind};

    fn layout_of(block: &model::BlockNode) -> BlockLayout {
        let mut world = BridgeWorld::new();
        let (snippet, map) = serialize_block(block);
        let (src, offset) = block_document(&snippet, 300.0);
        let doc = compile(&mut world, &src).unwrap();
        raw_layout(&world, &doc.pages()[0], offset).resolve(&map)
    }

    #[test]
    fn math_nodes_get_nested_boxes() {
        let mut d = Document::empty();
        let a = d.ident("a");
        let b = d.ident("b");
        let (aid, bid) = (a.id, b.id);
        let f = d.frac(vec![a], vec![b]);
        let fid = f.id;
        let MathKind::Frac { num, den } = &f.kind else {
            unreachable!()
        };
        let (nid, did) = (num.id, den.id);
        let block = d.display(vec![f]);
        let l = layout_of(&block);

        let (fa, fb, ff) = (
            l.box_of(aid).unwrap(),
            l.box_of(bid).unwrap(),
            l.box_of(fid).unwrap(),
        );
        assert!(
            fa.bottom() <= fb.y + 1.0,
            "numerator above denominator: {fa:?} {fb:?}"
        );
        assert!(ff.contains(fa.x + fa.w / 2.0, fa.y + fa.h / 2.0));
        assert!(ff.contains(fb.x + fb.w / 2.0, fb.y + fb.h / 2.0));
        assert_eq!(l.box_of(nid), Some(fa));
        assert_eq!(l.box_of(did), Some(fb));
        assert_eq!(l.hit(fb.x + fb.w / 2.0, fb.y + fb.h / 2.0), Some(bid));
        // Display math is centred on the 300pt page.
        assert!(ff.x > 100.0 && ff.right() < 200.0);
    }

    #[test]
    fn empty_slots_have_geometry() {
        let mut d = Document::empty();
        let f = d.frac(vec![], vec![]);
        let MathKind::Frac { num, den } = &f.kind else {
            unreachable!()
        };
        let (nid, did) = (num.id, den.id);
        let block = d.display(vec![f]);
        let l = layout_of(&block);
        let (n, dn) = (
            l.box_of(nid).expect("numerator slot box"),
            l.box_of(did).expect("denominator slot box"),
        );
        assert!(n.w > 2.0 && n.h > 2.0, "{n:?}");
        assert!(n.bottom() <= dn.y + 1.0);
    }

    /// Paired brackets grow with their content, as in plain Typst.
    #[test]
    fn brackets_scale_around_fractions() {
        let height_of_open = |tall: bool, open: &str, close: &str| {
            let mut d = Document::empty();
            let inner = if tall {
                let (a, b) = (d.ident("a"), d.ident("b"));
                d.frac(vec![a], vec![b])
            } else {
                d.ident("a")
            };
            let (o, c) = (d.sym(open), d.sym(close));
            let oid = o.id;
            let block = d.display(vec![o, inner, c]);
            layout_of(&block).box_of(oid).expect("bracket box").h
        };
        for (open, close) in [("(", ")"), ("[", "]"), ("{", "}"), ("[", ")")] {
            let (small, big) = (
                height_of_open(false, open, close),
                height_of_open(true, open, close),
            );
            assert!(big > 1.4 * small, "{open}{close}: {small} → {big}");
        }
    }

    /// Operator names (`log`, `lim`) come from Typst's library; their glyphs
    /// must still map back to the node.
    #[test]
    fn operators_have_boxes() {
        for name in ["log", "lim", "sin", "max"] {
            let mut d = Document::empty();
            let (x, op) = (d.ident("x"), d.ident(name));
            let (xid, oid) = (x.id, op.id);
            let block = d.display(vec![x, op]);
            let l = layout_of(&block);
            let (bx, bo) = (l.box_of(xid).unwrap(), l.box_of(oid).expect(name));
            assert!(bo.x > bx.right() - 0.1 && bo.w > 5.0, "{name}: {bo:?}");
        }
    }

    #[test]
    fn text_glyphs_map_to_chars() {
        let mut d = Document::empty();
        let t = d.text("a*b c");
        let tid = t.id;
        let block = d.paragraph(vec![t]);
        let l = layout_of(&block);
        let chars: Vec<_> = l
            .glyphs
            .iter()
            .filter(|g| g.node == tid)
            .map(|g| g.chars.clone())
            .collect();
        assert_eq!(chars, vec![0..1, 1..2, 2..3, 3..4, 4..5]);
        let xs: Vec<_> = l.glyphs.iter().map(|g| g.rect.x).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
        assert!(l.height > 5.0 && l.baseline > 0.0);
    }
}
