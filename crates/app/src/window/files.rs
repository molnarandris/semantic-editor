//! File actions of a window: open, save, export. File dialogs go through
//! the portal, so the Flatpak needs no filesystem permissions.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use typst_bridge::{compile_pdf, serialize_document};

use super::Window;
use crate::file;

impl Window {
    /// Ask what to do with unsaved changes. True if the caller may go on.
    pub(super) async fn confirm_discard(self: &Rc<Self>) -> bool {
        self.apply_block_edit();
        if !self.is_dirty() {
            return true;
        }
        let dialog = adw::AlertDialog::new(
            Some("Save Changes?"),
            Some(&format!(
                "“{}” has unsaved changes. Changes which are not saved will be lost.",
                self.stem()
            )),
        );
        dialog.add_responses(&[
            ("cancel", "_Cancel"),
            ("discard", "_Discard"),
            ("save", "_Save"),
        ]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        match dialog.choose_future(Some(&self.window)).await.as_str() {
            "discard" => true,
            "save" => self.save().await,
            _ => false,
        }
    }

    pub(super) async fn open(self: Rc<Self>) {
        if !self.confirm_discard().await {
            return;
        }
        let dialog = gtk::FileDialog::builder()
            .title("Open Document")
            .filters(&filters("Semantic documents", "sem"))
            .build();
        if let Ok(file) = dialog.open_future(Some(&self.window)).await {
            self.load(file).await;
        }
    }

    pub(super) async fn load(self: &Rc<Self>, file: gio::File) {
        let result = match file.load_contents_future().await {
            Ok((bytes, _)) => std::str::from_utf8(&bytes)
                .map_err(|_| "the file is not UTF-8 text".to_string())
                .and_then(file::from_json),
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(doc) => {
                self.close_block_editor();
                self.set_document(doc, Some(file));
            }
            Err(e) => self.toast(&format!("Could not open the file: {e}")),
        }
    }

    /// Save to the current file, or ask for one. True if saved.
    pub(super) async fn save(self: &Rc<Self>) -> bool {
        let current = self.file.borrow().clone();
        match current {
            Some(file) => self.write(file).await,
            None => self.save_as().await,
        }
    }

    pub(super) async fn save_as(self: &Rc<Self>) -> bool {
        match self
            .choose_target("Save Document", "Semantic documents", "sem")
            .await
        {
            Some(file) => self.write(file).await,
            None => false,
        }
    }

    async fn write(self: &Rc<Self>, file: gio::File) -> bool {
        self.apply_block_edit();
        let blocks = self.editor.document().blocks.clone();
        let json = file::to_json(&self.editor.document());
        match write_file(&file, json.into_bytes()).await {
            Ok(()) => {
                *self.saved.borrow_mut() = blocks;
                *self.file.borrow_mut() = Some(file);
                self.update_title();
                true
            }
            Err(e) => {
                self.toast(&format!("Could not save: {e}"));
                false
            }
        }
    }

    /// A save dialog for a file named after the document with `extension`.
    async fn choose_target(&self, title: &str, kind: &str, extension: &str) -> Option<gio::File> {
        let dialog = gtk::FileDialog::builder()
            .title(title)
            .initial_name(format!("{}.{extension}", self.stem()))
            .filters(&filters(kind, extension))
            .build();
        dialog.save_future(Some(&self.window)).await.ok()
    }

    pub(super) async fn export_typ(self: Rc<Self>) {
        let Some(file) = self
            .choose_target("Export Typst", "Typst files", "typ")
            .await
        else {
            return;
        };
        self.apply_block_edit();
        let source = serialize_document(&self.editor.document());
        self.finish_export(&file, write_file(&file, source.into_bytes()).await);
    }

    pub(super) async fn export_pdf(self: Rc<Self>) {
        let Some(file) = self
            .choose_target("Export PDF", "PDF documents", "pdf")
            .await
        else {
            return;
        };
        self.apply_block_edit();
        let source = serialize_document(&self.editor.document());
        // A full-document compile can take a while: off the main thread.
        let result = match gio::spawn_blocking(move || compile_pdf(&source)).await {
            Ok(Ok(pdf)) => write_file(&file, pdf).await,
            Ok(Err(errors)) => Err(format!(
                "Typst reported an error: {}",
                errors.first().map_or("unknown", String::as_str)
            )),
            Err(_) => Err("the compiler crashed".into()),
        };
        self.finish_export(&file, result);
    }

    fn finish_export(&self, file: &gio::File, result: Result<(), String>) {
        match result {
            Ok(()) => {
                let name = file
                    .basename()
                    .map_or_else(String::new, |p| p.to_string_lossy().into_owned());
                self.toast(&format!("Exported “{name}”"));
            }
            Err(e) => self.toast(&format!("Export failed: {e}")),
        }
    }
}

async fn write_file(file: &gio::File, bytes: Vec<u8>) -> Result<(), String> {
    file.replace_contents_future(
        bytes,
        None,
        false,
        gio::FileCreateFlags::REPLACE_DESTINATION,
    )
    .await
    .map(|_| ())
    .map_err(|(_, e)| e.to_string())
}

fn filters(kind: &str, extension: &str) -> gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(kind));
    filter.add_suffix(extension);
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&filter);
    store
}
