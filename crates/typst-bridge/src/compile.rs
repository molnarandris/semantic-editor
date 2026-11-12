//! Per-block compilation and rasterization.

use std::sync::Arc;

use typst_layout::{Page, PagedDocument};

use crate::serialize::PREAMBLE;
use crate::world::BridgeWorld;

/// Transparent margin around every block page, in points. Glyphs may extend
/// beyond the line box (descenders, big operators); the margin keeps them in
/// the raster. Layout coordinates are relative to the content, not the page.
pub const BLOCK_PAD_PT: f64 = 8.0;

/// Typst code compiled around a block's snippet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Around {
    /// The document header and the code blocks before the block: definitions
    /// and rules in force. Shared between blocks (it can be large).
    pub header: Arc<str>,
    /// Right before the snippet (e.g. restoring counters).
    pub prefix: String,
    /// Right after the snippet (e.g. reporting counters).
    pub suffix: String,
}

/// Build the standalone document for one block whose content is `width_pt`
/// wide. Returns the source and the byte offset at which `snippet` starts,
/// to shift the block's span map by. The page is set up after the header,
/// so page settings there (paper, margins, page numbers) do not apply.
pub fn block_document(around: &Around, snippet: &str, width_pt: f64) -> (String, usize) {
    let page = width_pt + 2.0 * BLOCK_PAD_PT;
    let mut src = format!(
        "{PREAMBLE}{}\n#set page(width: {page}pt, height: auto, margin: {BLOCK_PAD_PT}pt, \
         fill: none, header: none, footer: none, background: none, foreground: none, columns: 1)\n{}",
        around.header, around.prefix
    );
    let offset = src.len();
    src.push_str(snippet);
    src.push_str(&around.suffix);
    (src, offset)
}

/// Typst code drawing a heading as an entry of an outline (the editor's
/// outline pane): its number and title, indented by level, level 1 bold,
/// at a fixed size (whatever the header makes of headings) and in `color`
/// (Typst code, `rgb("#202020")`). Lines span ascender to descender, so
/// every entry's line box has the same height and the page holds its ink.
pub fn outline_entry_rule(color: &str) -> String {
    format!(
        "#show heading: it => pad(left: (it.level - 1) * 1em, {{
  set text(size: 11pt, fill: {color}, weight: if it.level == 1 {{ \"bold\" }} else {{ \"regular\" }},
    top-edge: \"ascender\", bottom-edge: \"descender\")
  set par(justify: false, leading: 0.25em)
  if it.numbering != none {{ context counter(heading).display(it.numbering); h(0.5em) }}
  it.body
}})
"
    )
}

/// Compile a complete Typst document. Errors are returned as messages.
pub fn compile(world: &mut BridgeWorld, text: &str) -> Result<PagedDocument, Vec<String>> {
    world.set_main_text(text);
    typst::compile::<PagedDocument>(&*world)
        .output
        .map_err(|errors| errors.iter().map(|e| e.message.to_string()).collect())
}

/// An RGBA8 (premultiplied) raster of one page.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Raster({}x{})", self.width, self.height)
    }
}

impl Raster {
    /// The smallest rectangle `(x, y, width, height)` holding every pixel
    /// with ink (alpha > 0), or `None` if the raster is blank.
    pub fn ink_bounds(&self) -> Option<(u32, u32, u32, u32)> {
        let (w, h) = (self.width as usize, self.height as usize);
        let inked = |x: usize, y: usize| self.data[(y * w + x) * 4 + 3] > 0;
        let row_inked = |y: usize| (0..w).any(|x| inked(x, y));
        let top = (0..h).find(|&y| row_inked(y))?;
        let bottom = (0..h).rev().find(|&y| row_inked(y))?;
        let col_inked = |x: usize| (top..=bottom).any(|y| inked(x, y));
        let left = (0..w).find(|&x| col_inked(x))?;
        let right = (0..w).rev().find(|&x| col_inked(x))?;
        Some((
            left as u32,
            top as u32,
            (right - left + 1) as u32,
            (bottom - top + 1) as u32,
        ))
    }

    /// A copy of the rectangle `(x, y, width, height)`.
    pub fn crop(&self, (x, y, cw, ch): (u32, u32, u32, u32)) -> Raster {
        let stride = self.width as usize * 4;
        let mut data = Vec::with_capacity(cw as usize * ch as usize * 4);
        for row in y as usize..(y + ch) as usize {
            let start = row * stride + x as usize * 4;
            data.extend_from_slice(&self.data[start..start + cw as usize * 4]);
        }
        Raster {
            width: cw,
            height: ch,
            data,
        }
    }
}

/// Rasterize a page at `pixel_per_pt` (zoom × device scale).
pub fn render_page(page: &Page, pixel_per_pt: f64) -> Raster {
    let opts = typst_render::RenderOptions {
        pixel_per_pt: pixel_per_pt.into(),
        render_bleed: false,
    };
    let pixmap = typst_render::render(page, &opts);
    Raster {
        width: pixmap.width(),
        height: pixmap.height(),
        data: pixmap.take(),
    }
}

/// Compile a whole document (e.g. from `serialize_document`) and export it as
/// PDF. Uses its own world, so it can run on any thread.
pub fn compile_pdf(source: &str) -> Result<Vec<u8>, Vec<String>> {
    let mut world = BridgeWorld::new();
    let doc = compile(&mut world, source)?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default())
        .map_err(|errors| errors.iter().map(|e| e.message.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_bounds_and_crop() {
        let (w, h) = (6u32, 5u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        for (x, y) in [(2, 1), (4, 3)] {
            data[((y * w + x) * 4 + 3) as usize] = 255;
        }
        let raster = Raster {
            width: w,
            height: h,
            data,
        };
        let bounds = raster.ink_bounds();
        assert_eq!(bounds, Some((2, 1, 3, 3)));
        let cropped = raster.crop(bounds.unwrap());
        assert_eq!((cropped.width, cropped.height), (3, 3));
        assert_eq!(cropped.data[3], 255);
        assert_eq!(cropped.data[(2 * 3 + 2) * 4 + 3], 255);
        let blank = Raster {
            width: 2,
            height: 2,
            data: vec![0; 16],
        };
        assert_eq!(blank.ink_bounds(), None);
    }

    #[test]
    fn formula_compiles_and_renders_ink() {
        let mut world = BridgeWorld::new();
        let doc = compile(
            &mut world,
            &block_document(&Around::default(), "$ S <= (log chi)/(2) + c $", 300.0).0,
        )
        .expect("compiles");
        let raster = render_page(&doc.pages()[0], 2.0);
        assert_eq!(raster.width, (2.0 * (300.0 + 2.0 * BLOCK_PAD_PT)) as u32);
        let dark = raster.data.chunks(4).filter(|px| px[0] < 128).count();
        assert!(dark > 200, "only {dark} dark pixels");
    }

    /// An outline entry: the number (from the counters before it), the
    /// title, its own size whatever the header sets for headings.
    #[test]
    fn outline_entries() {
        let ink = |header: &str, prefix: &str, snippet: &str| {
            let mut world = BridgeWorld::new();
            let around = Around {
                header: header.into(),
                prefix: format!("{prefix}{}", outline_entry_rule("rgb(\"#202020\")")),
                suffix: String::new(),
            };
            let doc =
                compile(&mut world, &block_document(&around, snippet, 200.0).0).expect("compiles");
            let raster = render_page(&doc.pages()[0], 2.0);
            let (_, _, w, h) = raster.ink_bounds().expect("ink");
            (w, h)
        };
        let numbered = "#set heading(numbering: \"1.1\")";
        let plain = ink("", "", "= The $x$ map");
        let big = ink("#show heading: set text(size: 30pt)", "", "= The $x$ map");
        assert_eq!(plain, big);
        let first = ink(numbered, "", "= The $x$ map");
        // The page is the line box: as high with descenders as without.
        let page = |snippet: &str| {
            let mut world = BridgeWorld::new();
            let around = Around {
                prefix: outline_entry_rule("black"),
                ..Around::default()
            };
            let doc =
                compile(&mut world, &block_document(&around, snippet, 200.0).0).expect("compiles");
            doc.pages()[0].frame.height().to_pt()
        };
        assert!((page("= Name") - page("= Typing")).abs() < 0.01);
        assert!(first.0 > plain.0 + 10, "{first:?} {plain:?}");
        // The counters before it: 1.1 is narrower than 12.1.
        let later = ink(
            numbered,
            "#counter(heading).update((12,))\n",
            "== The $x$ map",
        );
        let early = ink(
            numbered,
            "#counter(heading).update((1,))\n",
            "== The $x$ map",
        );
        assert!(later.0 > early.0, "{later:?} {early:?}");
    }

    #[test]
    fn errors_are_reported() {
        let mut world = BridgeWorld::new();
        assert!(compile(&mut world, "#undefined-thing").is_err());
    }
}
