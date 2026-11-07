//! `SemEditor`: a custom GTK widget that shows a [`Document`] as a stack of
//! Typst-rendered block textures, draws the caret on top and turns input
//! into editing commands of the model. Compiles run on a background thread;
//! the widget never blocks.

mod caret;
mod imp;
mod jobs;

pub use imp::DEFAULT_ZOOM;

use std::cell::Ref;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use model::{Crumb, CursorPath, Document, NewBlock, NodeId};

glib::wrapper! {
    pub struct SemEditor(ObjectSubclass<imp::SemEditor>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl Default for SemEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl SemEditor {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Replace the document and re-render every block.
    pub fn set_document(&self, doc: Document) {
        self.imp().set_document(doc);
    }

    pub fn document(&self) -> Ref<'_, Document> {
        Ref::map(self.imp().editor.borrow(), |e| e.doc())
    }

    /// Logical pixels per point (1.0 = 100 %).
    pub fn zoom(&self) -> f64 {
        self.imp().zoom()
    }

    pub fn set_zoom(&self, zoom: f64) {
        self.imp().set_zoom(zoom);
    }

    /// Zoom in (positive) or out by `steps` steps of 10 %.
    pub fn zoom_by(&self, steps: f64) {
        self.imp().zoom_by(steps);
    }

    pub fn caret(&self) -> CursorPath {
        self.imp().editor.borrow().caret().clone()
    }

    /// The innermost structure (or formula) around the caret, if in math.
    pub fn current_node(&self) -> Option<NodeId> {
        self.imp().editor.borrow().context_ids().last().copied()
    }

    /// Breadcrumb of the caret, e.g. paragraph › math › fraction › numerator.
    pub fn breadcrumb(&self) -> Vec<Crumb> {
        let ed = self.imp().editor.borrow();
        ed.doc().breadcrumb(ed.caret())
    }

    /// A key hint for the caret's context.
    pub fn hint(&self) -> &'static str {
        self.imp().editor.borrow().hint()
    }

    /// Select the structure a breadcrumb entry stands for.
    pub fn select_crumb(&self, crumb: &Crumb) {
        self.imp().select_crumb(crumb.path_len);
    }

    /// Insert a diagram or raw Typst block after the caret's block.
    pub fn insert_block(&self, kind: NewBlock) {
        let imp = self.imp();
        if imp.editor.borrow_mut().insert_block(kind) {
            imp.after_command(true);
        }
    }

    /// The source of a raw/diagram block.
    pub fn block_source(&self, block: NodeId) -> Option<String> {
        self.imp()
            .editor
            .borrow()
            .block_source(block)
            .map(Into::into)
    }

    /// Replace the source of a raw/diagram block and re-render it.
    pub fn set_block_source(&self, block: NodeId, source: &str) {
        let imp = self.imp();
        if imp.editor.borrow_mut().set_block_source(block, source) {
            imp.after_command(true);
        }
    }

    /// The error of a block's last compile, if it failed.
    pub fn block_error(&self, block: NodeId) -> Option<String> {
        self.imp().block_error(block)
    }

    /// A block's current render (the last good one if its last compile
    /// failed) and the logical pixels per texture pixel at the current zoom.
    pub fn block_texture(&self, block: NodeId) -> Option<(gtk::gdk::Texture, f64)> {
        self.imp().block_texture(block)
    }

    /// A raw/diagram block should be opened for editing (Enter or double
    /// click on it).
    pub fn connect_edit_source<F: Fn(&Self, NodeId) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("edit-source", false, move |args| {
            let obj = args[0].get::<Self>().expect("SemEditor");
            f(&obj, NodeId(args[1].get::<u64>().expect("block id")));
            None
        })
    }

    /// A block's render or compile error arrived.
    pub fn connect_block_rendered<F: Fn(&Self, NodeId) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("block-rendered", false, move |args| {
            let obj = args[0].get::<Self>().expect("SemEditor");
            f(&obj, NodeId(args[1].get::<u64>().expect("block id")));
            None
        })
    }

    /// Called after every command (caret, selection or document changed).
    pub fn connect_caret_moved<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("caret-moved", false, move |args| {
            let obj = args[0].get::<Self>().expect("SemEditor");
            f(&obj);
            None
        })
    }
}
