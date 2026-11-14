//! File actions of a window: open, save, export. File dialogs go through
//! the portal, so the Flatpak needs no filesystem permissions.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use typst_bridge::{compile_pdf, serialize_document};

use model::Files;

use crate::history::History;

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
            .filters(&filters("Semantic documents and Typst files", "sem typ"))
            .build();
        if let Ok(file) = dialog.open_future(Some(&self.window)).await {
            self.load(file).await;
        }
    }

    /// Open a `.sem` file, or import a `.typ` file with the files it names
    /// (copied into the project): its document is saved as a new `.sem`
    /// file named after it.
    pub(super) async fn load(self: &Rc<Self>, file: gio::File) {
        let path = file.basename().unwrap_or_default();
        let typst = path.extension().is_some_and(|e| e == "typ");
        let result = match file.load_contents_future().await {
            Ok((bytes, _)) if typst => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => {
                    let files = self.local_files(&file, &text).await;
                    Ok((typst_bridge::import_project(&text, files), None, vec![]))
                }
                Err(_) => Err("the file is not UTF-8 text".to_string()),
            },
            Ok((bytes, _)) => file::from_zip(&bytes).map(|(doc, h, chat)| (doc, Some(h), chat)),
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok((doc, _, _)) if typst => {
                self.close_block_editor();
                self.set_document(doc, None, None);
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                *self.imported.borrow_mut() = stem;
                self.update_title();
            }
            Ok((doc, history, chat)) => {
                self.close_block_editor();
                self.set_document(doc, Some(file), history);
                self.set_chat(chat);
            }
            Err(e) => self.toast(&format!("Could not open the file: {e}")),
        }
    }

    /// The files Typst `source` (of `file`) names, read from beside it. The
    /// Flatpak sandbox grants only the chosen file, so if some cannot be
    /// read, a folder dialog asks for its folder, which grants that. Files
    /// not found are reported.
    async fn local_files(&self, file: &gio::File, source: &str) -> Files {
        let mut files = Files::default();
        let mut missing = typst_bridge::local_files(source);
        async fn read(dir: &gio::File, paths: &mut Vec<String>, files: &mut Files) {
            let mut left = vec![];
            for path in paths.drain(..) {
                match dir
                    .resolve_relative_path(&path)
                    .load_contents_future()
                    .await
                {
                    Ok((bytes, _)) => files.insert(path, bytes.to_vec()),
                    Err(_) => left.push(path),
                }
            }
            *paths = left;
        }
        if let Some(dir) = file.parent() {
            read(&dir, &mut missing, &mut files).await;
        }
        if missing.is_empty() {
            return files;
        }
        let name = file
            .basename()
            .map_or_else(String::new, |p| p.to_string_lossy().into_owned());
        let dialog = gtk::FileDialog::builder()
            .title(format!("Folder of “{name}”, for Its Pictures and Files"))
            .accept_label("_Use Folder")
            .build();
        if let Some(dir) = file.parent() {
            dialog.set_initial_folder(Some(&dir));
        }
        if let Ok(dir) = dialog.select_folder_future(Some(&self.window)).await {
            read(&dir, &mut missing, &mut files).await;
        }
        if !missing.is_empty() {
            self.toast(&format!(
                "Not found beside the file: {}",
                missing.join(", ")
            ));
        }
        files
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
        let saved = super::saved_state(&self.editor.document());
        let chat = self.chat.messages();
        // Every save that changes something is a version.
        if self.history.borrow().is_none() {
            *self.history.borrow_mut() = History::new().ok();
        }
        let zip = {
            let history = self.history.borrow();
            if let Some(history) = history.as_ref() {
                let snapshot = file::snapshot(&self.editor.document());
                let message = self.commit_message.take();
                let message = message.as_deref().unwrap_or("Saved");
                if let Err(e) = history.commit(&snapshot, &author(), message) {
                    self.toast(&format!("Saved without a new version: {e}"));
                }
            }
            file::to_zip(&self.editor.document(), history.as_ref(), &chat)
        };
        match async { write_file(&file, zip?).await }.await {
            Ok(()) => {
                *self.saved.borrow_mut() = saved;
                *self.saved_chat.borrow_mut() = chat;
                *self.file.borrow_mut() = Some(file);
                *self.imported.borrow_mut() = None;
                self.update_title();
                self.update_versions();
                true
            }
            Err(e) => {
                self.toast(&format!("Could not save: {e}"));
                false
            }
        }
    }

    /// Choose a picture and insert a figure of it, the file copied into the
    /// project (with `scratch`: as a scratch file, not versioned).
    pub(super) async fn insert_picture(self: Rc<Self>, scratch: bool) {
        let title = if scratch {
            "Insert Scratch Picture"
        } else {
            "Insert Picture"
        };
        let dialog = gtk::FileDialog::builder()
            .title(title)
            .filters(&filters("Pictures", PICTURE_EXTENSIONS))
            .build();
        let Ok(file) = dialog.open_future(Some(&self.window)).await else {
            return;
        };
        let name = file
            .basename()
            .map_or_else(|| "picture".into(), |p| p.to_string_lossy().into_owned());
        match file.load_contents_future().await {
            Ok((bytes, _)) => {
                self.editor.insert_picture(&name, bytes.to_vec(), scratch);
                self.editor.grab_focus();
            }
            Err(e) => self.toast(&format!("Could not read the picture: {e}")),
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

    /// Export the document as a Typst project into a chosen folder:
    /// `main.typ` and the files it names, asking before replacing a
    /// `main.typ` there.
    pub(super) async fn export_typ(self: Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("Export Typst Project Into Folder")
            .build();
        let Ok(folder) = dialog.select_folder_future(Some(&self.window)).await else {
            return;
        };
        let main = folder.child("main.typ");
        if main.query_exists(None::<&gio::Cancellable>) && !self.confirm_replace(&folder).await {
            return;
        }
        self.apply_block_edit();
        let project = file::typst_project(&self.editor.document());
        let mut result = Ok(());
        for (path, bytes) in project {
            let target = folder.resolve_relative_path(&path);
            if let Some(parent) = target.parent() {
                // Already there is fine.
                let _ = parent.make_directory_with_parents(None::<&gio::Cancellable>);
            }
            result = write_file(&target, bytes).await;
            if result.is_err() {
                break;
            }
        }
        self.finish_export(&main, result);
    }

    /// Ask whether to replace the Typst project in `folder`.
    async fn confirm_replace(&self, folder: &gio::File) -> bool {
        let name = folder
            .basename()
            .map_or_else(String::new, |p| p.to_string_lossy().into_owned());
        let dialog = adw::AlertDialog::new(
            Some("Replace main.typ?"),
            Some(&format!(
                "“{name}” already has a main.typ. Exporting replaces it and the pictures of the same names."
            )),
        );
        dialog.add_responses(&[("cancel", "_Cancel"), ("replace", "_Replace")]);
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.choose_future(Some(&self.window)).await == "replace"
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
        let files = file::project_files(&self.editor.document());
        // A full-document compile can take a while: off the main thread.
        let result = match gio::spawn_blocking(move || compile_pdf(&source, files)).await {
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

/// The author of new versions: the account's full name (else its user
/// name), no email address.
pub(super) fn author() -> String {
    let real = gtk::glib::real_name().to_string_lossy().into_owned();
    match real.as_str() {
        "" | "Unknown" => gtk::glib::user_name().to_string_lossy().into_owned(),
        _ => real,
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

/// Picture formats Typst reads.
const PICTURE_EXTENSIONS: &str = "png jpg jpeg gif svg webp pdf PNG JPG JPEG";

/// A filter for files with the (space separated) `extensions`.
fn filters(kind: &str, extensions: &str) -> gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(kind));
    for extension in extensions.split(' ') {
        filter.add_suffix(extension);
    }
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&filter);
    store
}
