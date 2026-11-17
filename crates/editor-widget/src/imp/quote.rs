//! A remark's quotation for its card in the sidebar: the lines its range
//! covers with one line above and one below (a whole block's remark: the
//! block), cut from the blocks' renders, so it reads as in the paper.

use super::*;

/// A piece of a quotation: top-level block `index`, from `top` to `bottom`
/// (pt, block coordinates).
#[derive(Clone, Copy, PartialEq)]
struct Span {
    index: usize,
    top: f64,
    bottom: f64,
}

/// Space kept around the lines cut out, pt.
const QUOTE_PAD_PT: f64 = 2.0;

impl SemEditor {
    /// What remark `id`'s quotation shows: its pieces, top to bottom.
    fn quote_spans(&self, id: u64) -> Option<Vec<Span>> {
        let (range, block) = {
            let ed = self.editor.borrow();
            let doc = ed.doc();
            let remark = doc.remarks.iter().find(|r| r.id == id)?;
            (
                doc.remark_range(remark),
                doc.top_index(remark.anchor.block())?,
            )
        };
        let whole = |index: usize| {
            let views = self.views.borrow();
            let view = views.get(index)?;
            Some(Span {
                index,
                top: 0.0,
                bottom: view.height_pt(),
            })
        };
        let Some((first, last)) = range else {
            return Some(vec![whole(block)?]);
        };
        let (i, j) = {
            let ed = self.editor.borrow();
            let doc = ed.doc();
            (doc.top_index(first.block)?, doc.top_index(last.block)?)
        };
        let (top, _) = self.context_lines(i, &first)?;
        let (_, bottom) = self.context_lines(j, &last)?;
        let mut spans: Vec<Span> = (i..=j).filter_map(whole).collect();
        if let Some(s) = spans.first_mut() {
            s.top = top;
        }
        if let Some(s) = spans.last_mut() {
            s.bottom = bottom;
        }
        Some(spans)
    }

    /// The top of the line before the one `at` is on and the bottom of the
    /// line after it, in top-level block `index` (its edges if there is no
    /// such line).
    fn context_lines(&self, index: usize, at: &CursorPath) -> Option<(f64, f64)> {
        let stops = self.block_stops(index);
        let stop = stops.iter().find(|s| s.cursor == *at)?;
        let bands = line_bands(&stops);
        let mid = (stop.top + stop.bottom) / 2.0;
        let k = bands
            .iter()
            .position(|&(t, b)| (t..=b).contains(&mid))
            .unwrap_or(0);
        let height = self.views.borrow().get(index)?.height_pt();
        let top = k.checked_sub(1).map_or(0.0, |k| bands[k].0);
        let bottom = bands.get(k + 1).map_or(height, |b| b.1);
        Some((top, bottom))
    }

    /// What remark `id`'s quotation depends on (its pieces and their
    /// renders), to redo it only when this changes.
    pub(crate) fn remark_quote_key(&self, id: u64) -> Option<Vec<u64>> {
        let spans = self.quote_spans(id)?;
        let views = self.views.borrow();
        let mut key = vec![];
        for s in spans {
            let view = views.get(s.index)?;
            key.extend([
                view.id.0,
                view.shown,
                (s.top * 100.0) as u64,
                (s.bottom * 100.0) as u64,
            ]);
        }
        Some(key)
    }

    /// Remark `id`'s quotation: its pieces cut from the renders and stacked
    /// (the blocks' spacing between them), and its device pixels per pt.
    /// `None` before the blocks are rendered.
    pub(crate) fn remark_quote(&self, id: u64) -> Option<(gdk::Texture, f64)> {
        let spans = self.quote_spans(id)?;
        let views = self.views.borrow();
        let pad = typst_bridge::BLOCK_PAD_PT;
        let mut pieces = vec![];
        let mut ppp = 0.0_f64;
        for s in &spans {
            let view = views.get(s.index)?;
            let texture = view.texture.as_ref()?;
            ppp = ppp.max(view.texture_ppp);
            let px = |pt: f64| (pt * view.texture_ppp).round() as i64;
            let (w, h) = (texture.width() as i64, texture.height() as i64);
            let x0 = px(pad).clamp(0, w);
            let x1 = (w - px(pad)).clamp(x0, w);
            let y0 = px(s.top - QUOTE_PAD_PT + pad).clamp(0, h);
            let y1 = px(s.bottom + QUOTE_PAD_PT + pad).clamp(y0, h);
            pieces.push((texture.clone(), x0, x1, y0, y1));
        }
        let gap = (BLOCK_SPACING_PT * ppp).round() as i64;
        let width = pieces.iter().map(|p| p.2 - p.1).max()?;
        let height: i64 =
            pieces.iter().map(|p| p.4 - p.3).sum::<i64>() + gap * (pieces.len() as i64 - 1);
        if width <= 0 || height <= 0 {
            return None;
        }
        let stride = width as usize * 4;
        let mut out = vec![0u8; stride * height as usize];
        let mut row = 0usize;
        for (k, (texture, x0, x1, y0, y1)) in pieces.iter().enumerate() {
            if k > 0 {
                row += gap as usize;
            }
            let mut downloader = gdk::TextureDownloader::new(texture);
            downloader.set_format(gdk::MemoryFormat::R8g8b8a8Premultiplied);
            let (bytes, src_stride) = downloader.download_bytes();
            for y in *y0..*y1 {
                let src = y as usize * src_stride + *x0 as usize * 4;
                let len = (*x1 - *x0) as usize * 4;
                let dst = row * stride;
                out[dst..dst + len].copy_from_slice(&bytes[src..src + len]);
                row += 1;
            }
        }
        let texture = gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &glib::Bytes::from_owned(out),
            stride,
        );
        Some((texture.upcast(), ppp))
    }
}

/// The lines of a block as vertical bands (top, bottom), top to bottom:
/// the stops' lines, overlapping ones merged (a fraction's rows are one
/// line of text).
fn line_bands(stops: &[Stop]) -> Vec<(f64, f64)> {
    let mut bands: Vec<(f64, f64)> = caret::lines(stops)
        .iter()
        .map(|line| {
            let top = line
                .iter()
                .map(|&i| stops[i].top)
                .fold(f64::INFINITY, f64::min);
            let bottom = line
                .iter()
                .map(|&i| stops[i].bottom)
                .fold(f64::NEG_INFINITY, f64::max);
            (top, bottom)
        })
        .collect();
    bands.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = vec![];
    for (top, bottom) in bands {
        match merged.last_mut() {
            Some(last) if top < last.1 - 0.5 => last.1 = last.1.max(bottom),
            _ => merged.push((top, bottom)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(top: f64, bottom: f64, line: f64) -> Stop {
        Stop {
            cursor: CursorPath {
                block: NodeId(1),
                path: vec![],
                offset: 0,
            },
            x: 0.0,
            top,
            bottom,
            line,
            depth: 0,
            edge: false,
            label: false,
            tab: false,
        }
    }

    /// Lines are bands top to bottom; a fraction's rows, overlapping the
    /// text line around them, are one band with it.
    #[test]
    fn lines_as_bands() {
        let stops = [
            stop(20.0, 32.0, 30.0),
            stop(0.0, 12.0, 10.0),
            stop(36.0, 44.0, 42.0),
            stop(40.0, 52.0, 50.0),
        ];
        assert_eq!(
            line_bands(&stops),
            [(0.0, 12.0), (20.0, 32.0), (36.0, 52.0)]
        );
    }
}
