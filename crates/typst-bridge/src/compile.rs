//! Per-block compilation and rasterization.

use typst_layout::{Page, PagedDocument};

use crate::serialize::PREAMBLE;
use crate::world::BridgeWorld;

/// Transparent margin around every block page, in points. Glyphs may extend
/// beyond the line box (descenders, big operators); the margin keeps them in
/// the raster. Layout coordinates are relative to the content, not the page.
pub const BLOCK_PAD_PT: f64 = 8.0;

/// Build the standalone document for one block whose content is `width_pt`
/// wide. Returns the source and the byte offset at which `snippet` starts,
/// to shift the block's span map by.
pub fn block_document(snippet: &str, width_pt: f64) -> (String, usize) {
    let page = width_pt + 2.0 * BLOCK_PAD_PT;
    let mut src = format!(
        "#set page(width: {page}pt, height: auto, margin: {BLOCK_PAD_PT}pt, fill: none)\n{PREAMBLE}"
    );
    let offset = src.len();
    src.push_str(snippet);
    (src, offset)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_compiles_and_renders_ink() {
        let mut world = BridgeWorld::new();
        let doc = compile(
            &mut world,
            &block_document("$ S <= (log chi)/(2) + c $", 300.0).0,
        )
        .expect("compiles");
        let raster = render_page(&doc.pages()[0], 2.0);
        assert_eq!(raster.width, (2.0 * (300.0 + 2.0 * BLOCK_PAD_PT)) as u32);
        let dark = raster.data.chunks(4).filter(|px| px[0] < 128).count();
        assert!(dark > 200, "only {dark} dark pixels");
    }

    #[test]
    fn errors_are_reported() {
        let mut world = BridgeWorld::new();
        assert!(compile(&mut world, "#undefined-thing").is_err());
    }
}
