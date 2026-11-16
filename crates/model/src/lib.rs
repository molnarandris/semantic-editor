//! Semantic document model: tree, cursor, ops, undo, editing commands. Source of truth.
//! Depends on neither GTK nor Typst.

mod cursor;
mod editor;
mod files;
mod history;
mod ops;
mod remarks;
mod selection;
mod symbols;
mod tree;

pub use cursor::{Crumb, CursorPath, Step, Target};
pub use editor::{
    CallResolver, CallShape, DIAGRAM_TEMPLATE, Editor, INLINE_DIAGRAM_TEMPLATE, MathCallResolver,
    MathCallShape, NewBlock, Selected, SymbolResolver,
};
pub use files::{Files, SCRATCH_DIR, is_scratch};
pub use history::History;
pub use ops::{Items, Op, OpError, Seq};
pub use remarks::{Anchor, Point, Remark, Reply};
pub use selection::{Fragment, SelRange, Selection};
pub use symbols::OPERATORS;
pub use tree::{
    BARS, Block, BlockNode, Document, Inline, InlineNode, ListKind, MathKind, MathNode, MathRow,
    NodeId, OPTION_CALLS, Place, TitleFields, bib_keys, embedded_bib_keys, spaced_gap,
};
