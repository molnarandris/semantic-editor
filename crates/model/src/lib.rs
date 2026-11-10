//! Semantic document model: tree, cursor, ops, undo, editing commands. Source of truth.
//! Depends on neither GTK nor Typst.

mod cursor;
mod editor;
mod history;
mod ops;
mod selection;
mod symbols;
mod tree;

pub use cursor::{Crumb, CursorPath, Step, Target};
pub use editor::{
    CallResolver, CallShape, DIAGRAM_TEMPLATE, Editor, INLINE_DIAGRAM_TEMPLATE, MathCallResolver,
    MathCallShape, NewBlock, Selected, SymbolResolver,
};
pub use history::History;
pub use ops::{Items, Op, OpError, Seq};
pub use selection::{SelRange, Selection};
pub use tree::{
    BARS, Block, BlockNode, Document, Inline, InlineNode, ListKind, MathKind, MathNode, MathRow,
    NodeId, Place, TitleFields, bib_keys, embedded_bib_keys, spaced_gap,
};
