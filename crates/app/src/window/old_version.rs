//! An old version in a window of its own: read-only, a banner saying which
//! version it is, with "Restore this Version". Naming it there names that
//! version.

use std::rc::{Rc, Weak};

use adw::prelude::*;
use git2::Oid;
use gtk::gio;

use super::{Window, versions};
use crate::file;

/// What a read-only window shows: version `id` of the document in `window`.
#[derive(Clone)]
pub struct OldVersion {
    pub window: Weak<Window>,
    pub id: Oid,
    /// When it was saved, "Today, 14:03".
    pub label: String,
    /// The document's name.
    pub stem: String,
}

/// Window actions an old version has no use for.
const EDITING_ACTIONS: [&str; 12] = [
    "save",
    "save-as",
    "open",
    "open-sample",
    "edit-header",
    "insert-env",
    "insert-picture",
    "insert-scratch-picture",
    "insert-diagram",
    "insert-raw",
    "insert-bibliography",
    "insert-title",
];

impl Window {
    /// Open version `id` of this window's document read-only in a new
    /// window.
    pub(super) fn open_version(self: &Rc<Self>, id: Oid) {
        let found = {
            let history = self.history.borrow();
            let history = history.as_ref();
            let version = history
                .and_then(|h| h.versions().ok())
                .and_then(|vs| vs.into_iter().find(|v| v.id == id));
            let doc = history
                .ok_or_else(|| "no version history".to_string())
                .and_then(|h| h.files_at(id))
                .and_then(file::from_snapshot);
            version.map(|v| (versions::label(&v), doc))
        };
        let Some((label, doc)) = found else {
            return;
        };
        let doc = match doc {
            Ok(doc) => doc,
            Err(e) => return self.toast(&format!("Could not open the version: {e}")),
        };
        let Some(app) = self.window.application().and_downcast::<adw::Application>() else {
            return;
        };
        let view = Window::new(&app);
        *view.origin.borrow_mut() = Some(OldVersion {
            window: Rc::downgrade(self),
            id,
            label: label.clone(),
            stem: self.stem(),
        });
        view.editor.set_read_only(true);
        view.remarks.set_read_only(true);
        view.set_document(doc, None, None);
        *view.history.borrow_mut() = None;
        for name in EDITING_ACTIONS {
            let action = view.window.lookup_action(name);
            if let Some(action) = action.and_downcast::<gio::SimpleAction>() {
                action.set_enabled(false);
            }
        }
        view.sidebar_switcher.set_visible(false);
        // No chat about an old version.
        view.right_switcher.set_visible(false);
        view.banner
            .set_title(&format!("The version of {label}, read-only"));
        view.banner.set_revealed(true);
        view.update_title();
        view.present();
    }

    /// Make the version this read-only window shows the current one of its
    /// document, as a new version. Unsaved changes there are saved first,
    /// so nothing is lost.
    pub(super) async fn restore_version(self: Rc<Self>) {
        let Some(old) = self.origin.borrow().clone() else {
            return;
        };
        let Some(origin) = old.window.upgrade() else {
            return self.toast("The document's window is closed.");
        };
        if origin.is_dirty() && !origin.save().await {
            return;
        }
        let mut doc = self.editor.document().clone();
        // Scratch files are not in versions: the current ones stay.
        for (path, bytes) in origin.editor.document().files.iter() {
            if model::is_scratch(path) {
                doc.files.insert(path, bytes.to_vec());
            }
        }
        origin.editor.set_document(doc);
        *origin.commit_message.borrow_mut() =
            Some(format!("Restored the version of {}", old.label));
        if origin.save().await {
            origin.window.present();
            self.window.close();
        }
    }
}
