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

    /// Move the caret to the next block (or with `forward` false, the
    /// previous one) that has a problem, wrapping around: a failed compile,
    /// a reference to a missing label, a label defined twice. Returns the
    /// problem's place (from 1), their number and its message; `None` if
    /// there are none.
    pub fn go_to_problem(&self, forward: bool) -> Option<(usize, usize, String)> {
        let imp = self.imp();
        let problems = imp.problems();
        let index_of = |id: NodeId| {
            let ed = imp.editor.borrow();
            let doc = ed.doc();
            let top = doc.top_block(id).unwrap_or(id);
            doc.blocks.iter().position(|b| b.id == top)
        };
        let here = index_of(self.caret().block).unwrap_or(0);
        let at: Vec<usize> = problems
            .iter()
            .map(|(id, _)| index_of(*id).unwrap_or(0))
            .collect();
        let pick = if forward {
            at.iter().position(|&i| i > here).or(Some(0))
        } else {
            at.iter()
                .rposition(|&i| i < here)
                .or(at.len().checked_sub(1))
        };
        let k = pick.filter(|&k| k < problems.len())?;
        imp.go_to_block(problems[k].0);
        Some((k + 1, problems.len(), problems[k].1.clone()))
    }

    /// Put the caret at the start of top-level block `id` and show it.
    pub fn go_to_block(&self, id: NodeId) {
        self.imp().go_to_block(id);
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

    /// Start an environment: a paragraph with `#` for its name (`lemma`,
    /// then space).
    pub fn start_env(&self) {
        let imp = self.imp();
        if imp.editor.borrow_mut().start_env() {
            imp.after_command(true);
        }
    }

    /// What Ctrl+Enter would open in the source dialog: the caret's
    /// raw/diagram block or the code atom (code in a formula) next to it.
    pub fn source_target(&self) -> Option<NodeId> {
        self.imp().editor.borrow().source_target()
    }

    /// The source of a raw/diagram block or code atom.
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

    /// A raw/diagram block should be opened for editing (Ctrl+Enter or double
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
