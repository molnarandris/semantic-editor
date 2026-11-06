//! Undo/redo as stacks of inverse-op transactions.

use crate::cursor::CursorPath;
use crate::ops::{Op, OpError};
use crate::tree::Document;

/// One undoable step: the ops that undo it, and where the caret was before
/// and after so undo/redo can restore it.
#[derive(Clone, Debug)]
struct Txn {
    ops: Vec<Op>,
    before: CursorPath,
    after: CursorPath,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Txn>,
    redo: Vec<Txn>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply an edit as one undoable transaction. Clears the redo stack.
    pub fn apply(
        &mut self,
        doc: &mut Document,
        ops: Vec<Op>,
        before: CursorPath,
        after: CursorPath,
    ) -> Result<(), OpError> {
        let ops = doc.apply_all(ops)?;
        self.undo.push(Txn { ops, before, after });
        self.redo.clear();
        Ok(())
    }

    /// Apply an edit and merge it into the last transaction (consecutive
    /// keystrokes). Falls back to a new transaction if there is none.
    pub fn apply_merged(
        &mut self,
        doc: &mut Document,
        ops: Vec<Op>,
        after: CursorPath,
    ) -> Result<(), OpError> {
        let Some(last) = self.undo.last_mut() else {
            let before = after.clone();
            return self.apply(doc, ops, before, after);
        };
        let mut inverse = doc.apply_all(ops)?;
        // Undo the newer edit first.
        inverse.append(&mut last.ops);
        last.ops = inverse;
        last.after = after;
        self.redo.clear();
        Ok(())
    }

    /// Undo the last transaction; returns the caret to restore.
    pub fn undo(&mut self, doc: &mut Document) -> Option<CursorPath> {
        let txn = self.undo.pop()?;
        let ops = doc.apply_all(txn.ops).expect("undo ops apply");
        let caret = txn.before.clone();
        self.redo.push(Txn { ops, ..txn });
        Some(caret)
    }

    /// Redo the last undone transaction; returns the caret to restore.
    pub fn redo(&mut self, doc: &mut Document) -> Option<CursorPath> {
        let txn = self.redo.pop()?;
        let ops = doc.apply_all(txn.ops).expect("redo ops apply");
        let caret = txn.after.clone();
        self.undo.push(Txn { ops, ..txn });
        Some(caret)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
