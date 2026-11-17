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

    /// Refuse (or allow again) every change of the document, as for an old
    /// version shown; moving, selecting and copying still work.
    pub fn set_read_only(&self, read_only: bool) {
        self.imp().editor.borrow_mut().set_read_only(read_only);
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

    /// Heading `id` rendered by Typst as an outline entry (number and
    /// title), with its device pixels per logical pixel; `None` until its
    /// first render. See `connect_outline_rendered`.
    pub fn outline_entry(&self, id: NodeId) -> Option<(gtk::gdk::Texture, f64)> {
        self.imp().outline_entry(id)
    }

    /// Add a remark about the selection, or the top-level block the caret
    /// is in (see [`model::Editor::add_remark`]): its id, `None` if
    /// read-only.
    pub fn add_remark(&self, text: &str) -> Option<u64> {
        let added = self.imp().editor.borrow_mut().add_remark(text);
        if added.is_some() {
            self.imp().after_command(false);
        }
        added
    }

    /// The selection as Typst, as Copy puts it on the clipboard.
    pub fn selection_source(&self) -> Option<String> {
        let fragment = self.imp().editor.borrow().selection_fragment()?;
        Some(typst_bridge::fragment_source(&fragment))
    }

    /// Render Typst `markup` (a chat answer) `width_px` logical pixels wide,
    /// with the document's definitions, in the text colour; `done` gets
    /// the texture and its device pixels per logical pixel, or the errors.
    pub fn render_snippet(
        &self,
        markup: &str,
        width_px: f64,
        done: impl FnOnce(Result<(gtk::gdk::Texture, f64), String>) + 'static,
    ) {
        let width_pt = width_px / imp::SNIPPET_PX_PER_PT;
        let color = self.color();
        self.imp()
            .render_snippet(markup, width_pt, color, Box::new(done));
    }

    /// The open remarks the caret is in (see `Editor::remarks_at_caret`).
    pub fn remarks_at_caret(&self) -> Vec<u64> {
        self.imp().editor.borrow().remarks_at_caret()
    }

    /// Select remark `id`'s text (or go to its block) and show it.
    pub fn select_remark(&self, id: u64) {
        self.imp().select_remark(id);
    }

    /// Change remark `id`'s text.
    pub fn set_remark_text(&self, id: u64, text: &str) {
        let imp = self.imp();
        if imp.editor.borrow_mut().set_remark_text(id, text) {
            imp.after_command(false);
        }
    }

    /// Resolve or reopen remark `id`.
    pub fn set_remark_resolved(&self, id: u64, resolved: bool) {
        let imp = self.imp();
        if imp.editor.borrow_mut().set_remark_resolved(id, resolved) {
            imp.after_command(false);
        }
    }

    /// Delete remark `id`.
    pub fn delete_remark(&self, id: u64) {
        let imp = self.imp();
        if imp.editor.borrow_mut().delete_remark(id) {
            imp.after_command(false);
        }
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

    /// Insert a figure of the picture `bytes` (file name `name`), stored in
    /// the project (with `scratch`: as a scratch file, not versioned); the
    /// caret goes into its caption.
    pub fn insert_picture(&self, name: &str, bytes: Vec<u8>, scratch: bool) {
        let imp = self.imp();
        if imp.editor.borrow_mut().insert_picture(name, bytes, scratch) {
            imp.after_command(true);
        }
    }

    /// Put scratch file `path` under version control (see
    /// `Editor::keep_file`).
    pub fn keep_file(&self, path: &str) {
        let imp = self.imp();
        if imp.editor.borrow_mut().keep_file(path) {
            imp.after_command(false);
        }
    }

    /// Remove project file `path` if the document does not use it.
    pub fn delete_file(&self, path: &str) {
        let imp = self.imp();
        if imp.editor.borrow_mut().delete_file(path) {
            imp.after_command(false);
        }
    }

    /// Whether the document uses project file `path`.
    pub fn file_used(&self, path: &str) -> bool {
        self.imp().editor.borrow().file_used(path)
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

    /// A heading's outline entry was rendered (param: the heading).
    pub fn connect_outline_rendered<F: Fn(&Self, NodeId) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("outline-rendered", false, move |args| {
            let obj = args[0].get::<Self>().expect("SemEditor");
            f(&obj, NodeId(args[1].get::<u64>().expect("heading id")));
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
