//! `SemEditor`: a custom GTK widget that shows a [`Document`] as a stack of
//! Typst-rendered block textures, draws the caret on top and turns input
//! into editing commands of the model. Compiles run on a background thread;
//! the widget never blocks.

mod caret;
mod imp;
mod jobs;

pub use imp::DEFAULT_ZOOM;

/// What the source editor's preview shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewTarget {
    /// A raw Typst or diagram block, as on the page.
    Block(NodeId),
    /// A specimen of text styled by the document header.
    Header,
}

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

    /// Keep a render of `target` at `pixel_per_pt` device pixels per point,
    /// cropped to its ink, up to date for a preview (`None` stops it). Read
    /// it with [`Self::preview_texture`] and [`Self::preview_error`] on
    /// `preview-rendered`.
    pub fn set_preview(&self, target: Option<(PreviewTarget, f64)>) {
        self.imp().set_preview(target);
    }

    /// The preview render and its device pixels per point.
    pub fn preview_texture(&self) -> Option<(gtk::gdk::Texture, f64)> {
        self.imp().preview_texture()
    }

    /// The error of the preview's last compile, if it failed.
    pub fn preview_error(&self) -> Option<String> {
        self.imp().preview_error()
    }

    /// Show code blocks (raw Typst that only defines things) as their
    /// source instead of a marker.
    pub fn set_show_code(&self, show: bool) {
        self.imp().set_show_code(show);
    }

    /// The document header.
    pub fn header(&self) -> String {
        self.imp().editor.borrow().doc().header.clone()
    }

    /// Replace the document header (one undo step per run of edits) and
    /// re-render every block.
    pub fn set_header(&self, header: &str) {
        let imp = self.imp();
        if imp.editor.borrow_mut().set_header(header) {
            imp.after_command(false);
        }
    }

    /// The preview render or its error changed.
    pub fn connect_preview_rendered<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("preview-rendered", false, move |args| {
            let obj = args[0].get::<Self>().expect("SemEditor");
            f(&obj);
            None
        })
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

    /// Called after every command (caret, selection or document changed).
    pub fn connect_caret_moved<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("caret-moved", false, move |args| {
            let obj = args[0].get::<Self>().expect("SemEditor");
            f(&obj);
            None
        })
    }
}
