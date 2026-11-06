//! Bridge between the semantic document model and the Typst compiler.
//! All Typst calls of the application live in this crate.

pub mod compile;
pub mod layout;
pub mod serialize;
pub mod symbols;
pub mod worker;
pub mod world;

pub use compile::{BLOCK_PAD_PT, Raster, block_document, compile, render_page};
pub use layout::{BlockLayout, GlyphBox, RawLayout, Rect, raw_layout};
pub use serialize::{PREAMBLE, SpanMap, serialize_block, serialize_document};
pub use symbols::resolve_symbol;
pub use worker::{CompileJob, CompileResult, Rendered, Worker};
pub use world::BridgeWorld;

/// The pinned Typst version, recorded in `.sem` files and cache keys.
pub const TYPST_VERSION: &str = "0.15.1";
