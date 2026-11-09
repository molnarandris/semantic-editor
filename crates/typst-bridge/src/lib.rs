//! Bridge between the semantic document model and the Typst compiler.
//! All Typst calls of the application live in this crate.

pub mod calls;
pub mod code;
pub mod compile;
pub mod counters;
pub mod disk_cache;
pub mod layout;
pub mod serialize;
pub mod symbols;
pub mod worker;
pub mod world;

pub use calls::{call_shape, math_call_shape};
pub use code::{code_display, is_code};
pub use compile::{
    Around, BLOCK_PAD_PT, Raster, block_document, compile, compile_pdf, render_page,
};
pub use counters::{CounterState, chain_code, initial_state, tracked_counters};
pub use layout::{BlockLayout, GlyphBox, RawLayout, Rect, raw_layout};
pub use serialize::{
    CETZ_IMPORT, CETZ_VERSION, DEFAULT_HEADER, HEADER_SPECIMEN, PREAMBLE, SpanMap, Style,
    serialize_block, serialize_block_numbered, serialize_block_styled, serialize_document,
    serialize_document_mapped, serialize_math_node,
};
pub use symbols::resolve_symbol;
pub use worker::{CompileJob, CompileResult, Rendered, Worker};
pub use world::BridgeWorld;

/// The pinned Typst version, recorded in `.sem` files and cache keys.
pub const TYPST_VERSION: &str = "0.15.1";
