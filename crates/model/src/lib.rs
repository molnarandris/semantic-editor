//! Semantic document model: tree, cursor, ops, undo. Source of truth.
//! Depends on neither GTK nor Typst.

mod cursor;
mod history;
mod ops;
mod selection;
mod tree;

pub use cursor::{Crumb, CursorPath, Step, Target};
pub use history::History;
pub use ops::{Items, Op, OpError, Seq};
pub use selection::{SelRange, Selection};
pub use tree::{
    Block, BlockNode, Document, Inline, InlineNode, MathKind, MathNode, MathRow, NodeId,
};
