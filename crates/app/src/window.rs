//! One document window: header bar, editor, Typst source pane, status bar,
//! and the file actions (open, save, export). File dialogs go through the
//! portal, so the Flatpak needs no filesystem permissions.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use adw::prelude::*;
use editor_widget::SemEditor;
use gtk::{gdk, gio, glib};
use model::{BlockNode, CursorPath, Document, Target};
use sourceview5::prelude::*;
use typst_bridge::{SpanMap, compile_pdf, serialize_document, serialize_document_mapped};

use crate::{file, sample};

thread_local! {
    /// Open windows. Closures hold weak references; this keeps each window's
    /// state alive until the window is destroyed.
    static WINDOWS: RefCell<Vec<Rc<Window>>> = const { RefCell::new(vec![]) };
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    editor: SemEditor,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    split: adw::OverlaySplitView,
    source: sourceview5::View,
    file: RefCell<Option<gio::File>>,
    /// The blocks as last saved or loaded, to tell if there are changes.
    saved: RefCell<Vec<BlockNode>>,
    /// Text currently in the source pane (to skip redundant updates).
    shown_source: RefCell<String>,
}

fn spawn(future: impl Future<Output = ()> + 'static) {
    glib::spawn_future_local(future);
}

impl Window {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let title = adw::WindowTitle::new("Untitled", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let open = gtk::Button::builder()
            .label("Open")
            .action_name("win.open")
            .tooltip_text("Open a Document (Ctrl+O)")
            .build();
        let new = gtk::Button::builder()
            .icon_name("tab-new-symbolic")
            .action_name("app.new")
            .tooltip_text("New Window (Ctrl+N)")
            .build();
        header.pack_start(&open);
        header.pack_start(&new);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main_menu())
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        let source_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .action_name("win.source-pane")
            .tooltip_text("Typst Source (F9)")
            .build();
        header.pack_end(&menu_button);
        header.pack_end(&source_toggle);

        let editor = SemEditor::new();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&editor)
            .build();

        let source = source_view();
        let source_scroller = gtk::ScrolledWindow::builder().child(&source).build();
        let split = adw::OverlaySplitView::builder()
            .content(&scroller)
            .sidebar(&source_scroller)
            .sidebar_position(gtk::PackType::End)
            .show_sidebar(false)
            .min_sidebar_width(280.0)
            .max_sidebar_width(720.0)
            .sidebar_width_fraction(0.42)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));

        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&toasts));
        content.add_bottom_bar(&status_bar(&editor));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(1100)
            .default_height(780)
            .content(&content)
            .build();
        if app
            .application_id()
            .is_some_and(|id| id.ends_with(".Devel"))
        {
            window.add_css_class("devel");
        }

        let this = Rc::new(Self {
            window,
            editor,
            title,
            toasts,
            split,
            source,
            file: RefCell::new(None),
            saved: RefCell::new(vec![]),
            shown_source: RefCell::new(String::new()),
        });
        this.add_actions();
        this.connect_signals();
        this.set_document(Document::new(), None);
        WINDOWS.with(|w| w.borrow_mut().push(this.clone()));
        this
    }

    pub fn present(&self) {
        self.window.present();
        self.editor.grab_focus();
    }

    /// Open `file` in this window (no unsaved-changes check).
    pub fn open_file(self: &Rc<Self>, file: gio::File) {
        let this = self.clone();
        spawn(async move { this.load(file).await });
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.editor.connect_caret_moved(move |_| {
            if let Some(this) = weak.upgrade() {
                this.update_title();
                this.update_source();
            }
        });

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !this.is_dirty() {
                return glib::Propagation::Proceed;
            }
            spawn(async move {
                if this.confirm_discard().await {
                    this.window.destroy();
                }
            });
            glib::Propagation::Stop
        });

        let weak = Rc::downgrade(self);
        self.window.connect_destroy(move |_| {
            if let Some(this) = weak.upgrade() {
                WINDOWS.with(|w| w.borrow_mut().retain(|x| !Rc::ptr_eq(x, &this)));
            }
        });
    }

    fn add_actions(self: &Rc<Self>) {
        type Handler = fn(Rc<Window>);
        let actions: [(&str, Handler); 6] = [
            ("open", |w| spawn(async move { w.open().await })),
            ("save", |w| {
                spawn(async move {
                    w.save().await;
                })
            }),
            ("save-as", |w| {
                spawn(async move {
                    w.save_as().await;
                })
            }),
            ("export-pdf", |w| spawn(async move { w.export_pdf().await })),
            ("export-typ", |w| spawn(async move { w.export_typ().await })),
            ("open-sample", |w| {
                spawn(async move {
                    if w.confirm_discard().await {
                        w.set_document(sample::document(), None);
                    }
                })
            }),
        ];
        for (name, handler) in actions {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    handler(this);
                }
            });
            self.window.add_action(&action);
        }

        let pane = gio::SimpleAction::new_stateful("source-pane", None, &false.to_variant());
        let weak = Rc::downgrade(self);
        pane.connect_activate(move |action, _| {
            let Some(this) = weak.upgrade() else { return };
            let on = !action
                .state()
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            action.set_state(&on.to_variant());
            this.split.set_show_sidebar(on);
            this.update_source();
        });
        // The pane can also be closed by the split view itself (narrow window).
        let action = pane.clone();
        self.split.connect_show_sidebar_notify(move |split| {
            action.set_state(&split.shows_sidebar().to_variant());
        });
        self.window.add_action(&pane);
    }

    // ----- State -----

    fn set_document(&self, doc: Document, file: Option<gio::File>) {
        *self.saved.borrow_mut() = doc.blocks.clone();
        *self.file.borrow_mut() = file;
        // Emits caret-moved, which updates title, status bar and source.
        self.editor.set_document(doc);
    }

    fn is_dirty(&self) -> bool {
        self.editor.document().blocks != *self.saved.borrow()
    }

    /// File name without extension, for titles and export names.
    fn stem(&self) -> String {
        self.file
            .borrow()
            .as_ref()
            .and_then(|f| f.basename())
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Untitled".into())
    }

    fn update_title(&self) {
        let name = match self.file.borrow().as_ref().and_then(|f| f.basename()) {
            Some(path) => path.to_string_lossy().into_owned(),
            None => "Untitled".into(),
        };
        let shown = if self.is_dirty() {
            format!("• {name}")
        } else {
            name.clone()
        };
        self.title.set_title(&shown);
        self.window
            .set_title(Some(&format!("{shown} — Semantic Editor")));
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    // ----- Files -----

    /// Ask what to do with unsaved changes. True if the caller may go on.
    async fn confirm_discard(self: &Rc<Self>) -> bool {
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

    async fn open(self: Rc<Self>) {
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

    async fn load(self: &Rc<Self>, file: gio::File) {
        let result = match file.load_contents_future().await {
            Ok((bytes, _)) => std::str::from_utf8(&bytes)
                .map_err(|_| "the file is not UTF-8 text".to_string())
                .and_then(file::from_json),
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(doc) => self.set_document(doc, Some(file)),
            Err(e) => self.toast(&format!("Could not open the file: {e}")),
        }
    }

    /// Save to the current file, or ask for one. True if saved.
    async fn save(self: &Rc<Self>) -> bool {
        let current = self.file.borrow().clone();
        match current {
            Some(file) => self.write(file).await,
            None => self.save_as().await,
        }
    }

    async fn save_as(self: &Rc<Self>) -> bool {
        match self
            .choose_target("Save Document", "Semantic documents", "sem")
            .await
        {
            Some(file) => self.write(file).await,
            None => false,
        }
    }

    async fn write(self: &Rc<Self>, file: gio::File) -> bool {
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

    async fn export_typ(self: Rc<Self>) {
        let Some(file) = self
            .choose_target("Export Typst", "Typst files", "typ")
            .await
        else {
            return;
        };
        let source = serialize_document(&self.editor.document());
        self.finish_export(&file, write_file(&file, source.into_bytes()).await);
    }

    async fn export_pdf(self: Rc<Self>) {
        let Some(file) = self
            .choose_target("Export PDF", "PDF documents", "pdf")
            .await
        else {
            return;
        };
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

    // ----- Source pane -----

    /// Show the document's Typst source with the caret's position and its
    /// innermost structure marked. Only while the pane is open.
    fn update_source(&self) {
        if !self.split.shows_sidebar() {
            return;
        }
        let doc = self.editor.document();
        let (src, map) = serialize_document_mapped(&doc);
        let buffer = self.source.buffer();
        if *self.shown_source.borrow() != src {
            buffer.set_text(&src);
            *self.shown_source.borrow_mut() = src.clone();
        }
        let (start, end) = buffer.bounds();
        buffer.remove_tag_by_name("current", &start, &end);
        let chars = |byte: usize| src[..byte.min(src.len())].chars().count() as i32;
        if let Some(range) = self.editor.current_node().and_then(|id| map.range(id)) {
            let (a, b) = (
                buffer.iter_at_offset(chars(range.start)),
                buffer.iter_at_offset(chars(range.end)),
            );
            buffer.apply_tag_by_name("current", &a, &b);
        }
        if let Some(byte) = caret_byte(&doc, &map, &self.editor.caret()) {
            buffer.place_cursor(&buffer.iter_at_offset(chars(byte)));
            self.source.scroll_mark_onscreen(&buffer.get_insert());
        }
    }
}

/// The source byte offset that corresponds to a caret.
fn caret_byte(doc: &Document, map: &SpanMap, caret: &CursorPath) -> Option<usize> {
    match doc.resolve(caret)? {
        Target::Text { inline, .. } => map.text_byte_of(inline.id, caret.offset),
        Target::Raw { block, text } => {
            let start = map.range(block.id)?.start;
            let within = text
                .char_indices()
                .nth(caret.offset)
                .map_or(text.len(), |(i, _)| i);
            Some(start + within)
        }
        Target::Row { row } => match (row.nodes.get(caret.offset), row.nodes.last()) {
            (Some(node), _) => map.range(node.id).map(|r| r.start),
            (None, Some(last)) => map.range(last.id).map(|r| r.end),
            (None, None) => map.range(row.id).map(|r| r.start),
        },
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

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some("_Save"), Some("win.save"));
    file.append(Some("Save _As…"), Some("win.save-as"));
    menu.append_section(None, &file);
    let export = gio::Menu::new();
    export.append(Some("Export _PDF…"), Some("win.export-pdf"));
    export.append(Some("Export _Typst…"), Some("win.export-typ"));
    menu.append_section(None, &export);
    let other = gio::Menu::new();
    other.append(Some("Open S_ample Document"), Some("win.open-sample"));
    menu.append_section(None, &other);
    menu
}

/// Read-only view of the generated Typst source.
fn source_view() -> sourceview5::View {
    let buffer = sourceview5::Buffer::new(None);
    if let Some(lang) = sourceview5::LanguageManager::default().language("typst") {
        buffer.set_language(Some(&lang));
    }
    let style = adw::StyleManager::default();
    let set_scheme = {
        let buffer = buffer.clone();
        move |style: &adw::StyleManager| {
            let name = if style.is_dark() {
                "Adwaita-dark"
            } else {
                "Adwaita"
            };
            let scheme = sourceview5::StyleSchemeManager::default().scheme(name);
            buffer.set_style_scheme(scheme.as_ref());
        }
    };
    set_scheme(&style);
    style.connect_dark_notify(set_scheme);
    let tag = gtk::TextTag::builder()
        .name("current")
        .background_rgba(&gdk::RGBA::new(0.21, 0.52, 0.89, 0.22))
        .build();
    buffer.tag_table().add(&tag);
    let view = sourceview5::View::with_buffer(&buffer);
    view.set_editable(false);
    view.set_cursor_visible(true);
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(6);
    view
}

/// Breadcrumb of the caret (each entry selects its structure) and a key hint.
fn status_bar(editor: &SemEditor) -> gtk::Box {
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(6)
        .margin_end(12)
        .margin_top(2)
        .margin_bottom(2)
        .build();
    let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let hint = gtk::Label::builder()
        .hexpand(true)
        .xalign(1.0)
        .css_classes(["caption", "dim-label"])
        .build();
    bar.append(&crumbs);
    bar.append(&hint);

    let update = move |editor: &SemEditor| {
        while let Some(child) = crumbs.first_child() {
            crumbs.remove(&child);
        }
        for (i, crumb) in editor.breadcrumb().into_iter().enumerate() {
            if i > 0 {
                let sep = gtk::Label::builder()
                    .label("›")
                    .css_classes(["caption", "dim-label"])
                    .build();
                crumbs.append(&sep);
            }
            let button = gtk::Button::builder()
                .label(crumb.label)
                .css_classes(["flat", "caption"])
                .focusable(false)
                .tooltip_text("Select")
                .build();
            let weak = editor.downgrade();
            button.connect_clicked(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.select_crumb(&crumb);
                    editor.grab_focus();
                }
            });
            crumbs.append(&button);
        }
        hint.set_label(editor.hint());
    };
    update(editor);
    editor.connect_caret_moved(update);
    bar
}
