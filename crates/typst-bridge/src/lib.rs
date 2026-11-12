//! Bridge between the semantic document model and the Typst compiler.
//! All Typst calls of the application live in this crate.

pub mod bib;
pub mod calls;
pub mod code;
pub mod compile;
pub mod counters;
pub mod deserialize;
pub mod disk_cache;
pub mod layout;
pub mod serialize;
pub mod symbols;
pub mod worker;
pub mod world;

pub use calls::{call_shape, math_call_shape};
pub use code::{code_display, is_code, label_values_in_source, labels_in_source, refs_in_source};
pub use compile::{
    Around, BLOCK_PAD_PT, Raster, block_document, compile, compile_pdf, outline_entry_rule,
    render_page,
};
pub use counters::{
    CounterState, LabelTarget, chain_code, initial_state, label_report, missing_ref, ref_stub,
    stand_in, tracked_counters,
};
pub use deserialize::{import_project, import_typ, local_files, parse_fragment};
pub use layout::{BlockLayout, GlyphBox, RawLayout, Rect, raw_layout};
pub use serialize::{
    CETZ_IMPORT, CETZ_VERSION, DEFAULT_HEADER, HEADER_SPECIMEN, PREAMBLE, SpanMap, Style,
    fragment_source, serialize_block, serialize_block_numbered, serialize_block_styled,
    serialize_document, serialize_document_mapped, serialize_math_node,
};
pub use symbols::resolve_symbol;
pub use worker::{CompileJob, CompileResult, Rendered, Worker};
pub use world::BridgeWorld;

/// The pinned Typst version, recorded in `.sem` files and cache keys.
pub const TYPST_VERSION: &str = "0.15.1";
