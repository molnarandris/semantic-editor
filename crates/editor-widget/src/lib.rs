//! `SemEditor`: a custom GTK widget that shows a [`Document`] as a stack of
//! Typst-rendered block textures, draws the caret on top and turns input
//! into editing commands of the model. Compiles run on a background thread;
//! the widget never blocks.

mod caret;
mod imp;

use std::cell::Ref;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use model::{Crumb, Document};

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

    /// Called after every command (caret, selection or document changed).
    pub fn connect_caret_moved<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("caret-moved", false, move |args| {
            let obj = args[0].get::<Self>().expect("SemEditor");
            f(&obj);
            None
        })
    }
}
