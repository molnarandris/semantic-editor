//! One document window: header bar, outline pane, editor, status bar. File
//! actions live in `files`, the source editor for diagram and raw Typst
//! blocks in `block_editor`, the outline in `outline`.

mod block_editor;
mod chat;
mod files;
mod old_version;
mod outline;
mod remarks;
mod title_editor;
mod versions;

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use adw::prelude::*;
use editor_widget::{PreviewTarget, SemEditor};
use gtk::{gio, glib};
use model::{BlockNode, Document, Files, NewBlock, Remark};

use crate::history::History;
use crate::sample;
use block_editor::BlockEditor;
use chat::ChatPane;
use old_version::OldVersion;
use outline::Outline;
use remarks::{RemarkAction, RemarksPane, Shown};
use versions::VersionsPane;

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
    outline: Outline,
    versions: VersionsPane,
    remarks: RemarksPane,
    chat: Rc<ChatPane>,
    /// The sidebar on the right: Remarks and Chat.
    split_remarks: adw::OverlaySplitView,
    right_pages: adw::ViewStack,
    right_switcher: adw::InlineViewSwitcher,
    /// The sidebar's Outline / Versions switcher (hidden for an old
    /// version).
    sidebar_switcher: adw::InlineViewSwitcher,
    /// Says which old version a read-only window shows, with "Restore".
    banner: adw::Banner,
    /// In a read-only window: the old version shown.
    origin: RefCell<Option<OldVersion>>,
    /// The message of the next version saved ("Saved" if none).
    commit_message: RefCell<Option<String>>,
    file: RefCell<Option<gio::File>>,
    /// The name (without extension) of the `.typ` file the document was
    /// opened from, until it is saved as a `.sem` file.
    imported: RefCell<Option<String>>,
    /// Header, blocks and files as last saved or loaded, to tell if there are
    /// changes.
    saved: RefCell<(String, Vec<BlockNode>, Files, Vec<Remark>)>,
    /// The document's version history (`None` if it could not be set up:
    /// saving then works without one).
    history: RefCell<Option<History>>,
    /// The open source editor dialog, if any.
    block_editor: RefCell<Option<Rc<BlockEditor>>>,
    /// The toast of the last problem gone to, replaced by the next one.
    problem_toast: RefCell<Option<adw::Toast>>,
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
        let show_outline = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text("Outline")
            .build();
        header.pack_start(&show_outline);
        header.pack_start(&open);
        header.pack_start(&new);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main_menu())
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        header.pack_end(&menu_button);
        let show_remarks = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("Remarks and Chat")
            .build();
        header.pack_end(&show_remarks);

        let editor = SemEditor::new();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&editor)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scroller));

        // On the right: the remarks and the chat, a switcher on top.
        let remarks = RemarksPane::new();
        let chat = ChatPane::new();
        let right_pages = adw::ViewStack::builder().vexpand(true).build();
        right_pages.add_titled(&remarks.widget, Some("remarks"), "Remarks");
        right_pages.add_titled(&chat.widget, Some("chat"), "Chat");
        let right_switcher = adw::InlineViewSwitcher::builder()
            .stack(&right_pages)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.append(&right_switcher);
        right.append(&right_pages);
        let split_remarks = adw::OverlaySplitView::builder()
            .sidebar(&right)
            .sidebar_position(gtk::PackType::End)
            .content(&toasts)
            // libadwaita's split views cannot be resized by dragging.
            .min_sidebar_width(320.0)
            .max_sidebar_width(520.0)
            .sidebar_width_fraction(0.4)
            .show_sidebar(false)
            .build();
        show_remarks
            .bind_property("active", &split_remarks, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();

        // The sidebar: the outline and the versions, a switcher on top.
        let outline = Outline::new();
        let versions = VersionsPane::new();
        let pages = adw::ViewStack::new();
        pages.add_titled(&outline.widget, Some("outline"), "Outline");
        pages.add_titled(&versions.widget, Some("versions"), "Versions");
        let switcher = adw::InlineViewSwitcher::builder()
            .stack(&pages)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let banner = adw::Banner::builder()
            .button_label("_Restore this Version")
            .revealed(false)
            .build();
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.append(&switcher);
        pages.set_vexpand(true);
        sidebar.append(&pages);
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&split_remarks)
            .max_sidebar_width(260.0)
            .build();
        show_outline
            .bind_property("active", &split, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();

        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.add_top_bar(&banner);
        content.set_content(Some(&split));
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
            outline,
            versions,
            remarks,
            chat,
            split_remarks,
            right_pages,
            right_switcher,
            sidebar_switcher: switcher,
            banner,
            origin: RefCell::new(None),
            commit_message: RefCell::new(None),
            file: RefCell::new(None),
            imported: RefCell::new(None),
            saved: RefCell::default(),
            history: RefCell::new(None),
            block_editor: RefCell::new(None),
            problem_toast: RefCell::new(None),
        });
        this.add_actions();
        this.connect_signals();
        this.set_document(new_document(), None, None);
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
                this.sync_block_editor();
                this.outline.update(&this.editor);
                this.update_scratch();
                this.update_remarks();
                this.chat.set_quote(this.selection_quote());
            }
        });
        let weak = Rc::downgrade(self);
        self.chat.connect_paper(move || match weak.upgrade() {
            Some(this) => {
                this.apply_block_edit();
                typst_bridge::serialize_document(&this.editor.document())
            }
            None => String::new(),
        });
        let weak = Rc::downgrade(self);
        self.chat.connect_render(move |markup, width, done| {
            if let Some(this) = weak.upgrade() {
                this.editor.render_snippet(markup, width, done);
            }
        });
        let weak = Rc::downgrade(self);
        self.remarks.connect_action(move |action| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            match action {
                // Showing a remark selects its range in the paper; that is
                // not a selection to ask the chat about.
                RemarkAction::Select(id) => {
                    this.editor.select_remark(id);
                    this.chat.set_quote(None);
                    this.editor.grab_focus();
                }
                RemarkAction::Show(id) => {
                    this.editor.select_remark(id);
                    this.chat.set_quote(None);
                }
                RemarkAction::SetText(id, text) => this.editor.set_remark_text(id, &text),
                RemarkAction::Resolve(id, resolved) => {
                    this.editor.set_remark_resolved(id, resolved)
                }
                RemarkAction::Delete(id) => this.editor.delete_remark(id),
                RemarkAction::FollowRef(label) => {
                    if this.editor.follow_ref(&label) {
                        this.editor.grab_focus();
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.editor.connect_block_rendered(move |_| {
            if let Some(this) = weak.upgrade() {
                this.refresh_quotes();
            }
        });
        let weak = Rc::downgrade(self);
        self.editor.connect_outline_rendered(move |editor, id| {
            if let Some(this) = weak.upgrade() {
                this.outline.show_render(editor, id);
            }
        });
        let weak = Rc::downgrade(self);
        self.versions.connect_pick(move |id| {
            if let Some(this) = weak.upgrade() {
                this.open_version(id);
            }
        });
        let weak = Rc::downgrade(self);
        self.versions.connect_keep(move |path| {
            if let Some(this) = weak.upgrade() {
                this.editor.keep_file(path);
            }
        });
        let weak = Rc::downgrade(self);
        self.versions.connect_delete(move |path| {
            if let Some(this) = weak.upgrade() {
                this.editor.delete_file(path);
            }
        });
        let weak = Rc::downgrade(self);
        self.banner.connect_button_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                spawn(async move { this.restore_version().await });
            }
        });
        let weak = Rc::downgrade(self);
        self.outline.connect_activated(move |id| {
            if let Some(this) = weak.upgrade() {
                this.editor.go_to_block(id);
                this.editor.grab_focus();
            }
        });
        let weak = Rc::downgrade(self);
        self.editor.connect_edit_source(move |_, block| {
            if let Some(this) = weak.upgrade() {
                this.open_block_editor(PreviewTarget::Block(block));
            }
        });
        let weak = Rc::downgrade(self);
        self.editor.connect_preview_rendered(move |_| {
            if let Some(this) = weak.upgrade() {
                this.refresh_block_preview();
            }
        });

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            this.apply_block_edit();
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
        let actions: [(&str, Handler); 22] = [
            ("insert-env", |w| {
                w.editor.start_env();
                w.editor.grab_focus();
            }),
            ("edit-header", |w| {
                w.open_block_editor(PreviewTarget::Header)
            }),
            ("insert-diagram", |w| w.insert_block(NewBlock::Diagram)),
            ("insert-picture", |w| {
                spawn(async move { w.insert_picture(false).await })
            }),
            ("insert-scratch-picture", |w| {
                spawn(async move { w.insert_picture(true).await })
            }),
            ("insert-raw", |w| w.insert_block(NewBlock::RawTypst)),
            ("insert-bibliography", |w| {
                w.insert_block(NewBlock::Bibliography)
            }),
            ("insert-title", |w| w.insert_block(NewBlock::Title)),
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
                        w.set_document(sample::document(), None, None);
                    }
                })
            }),
            ("add-remark", |w| w.add_remark()),
            ("ask-claude", |w| w.ask_claude()),
            ("name-version", |w| {
                spawn(async move { w.name_version(None).await })
            }),
            ("next-problem", |w| w.go_to_problem(true)),
            ("previous-problem", |w| w.go_to_problem(false)),
            ("zoom-in", |w| w.editor.zoom_by(1.0)),
            ("zoom-out", |w| w.editor.zoom_by(-1.0)),
            ("zoom-reset", |w| {
                w.editor.set_zoom(editor_widget::DEFAULT_ZOOM)
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

        let show_code = gio::SimpleAction::new_stateful("show-code", None, &false.to_variant());
        let weak = Rc::downgrade(self);
        show_code.connect_activate(move |action, _| {
            let show = !action
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(false);
            action.set_state(&show.to_variant());
            if let Some(this) = weak.upgrade() {
                this.editor.set_show_code(show);
            }
        });
        self.window.add_action(&show_code);
    }

    fn insert_block(self: Rc<Self>, kind: NewBlock) {
        self.editor.insert_block(kind);
        if let Some(target) = self.editor.source_target() {
            self.open_block_editor(PreviewTarget::Block(target));
        }
    }

    // ----- State -----

    /// Show `doc` (from `file`, with its `history`; a new history if none).
    fn set_document(&self, doc: Document, file: Option<gio::File>, history: Option<History>) {
        *self.history.borrow_mut() = history.or_else(|| History::new().ok());
        *self.saved.borrow_mut() = saved_state(&doc);
        *self.file.borrow_mut() = file;
        *self.imported.borrow_mut() = None;
        // Emits caret-moved, which updates the title and status bar.
        self.editor.set_document(doc);
        self.update_versions();
        let open = self.editor.document().remarks.iter().any(|r| !r.resolved);
        self.split_remarks.set_show_sidebar(open);
        self.right_pages.set_visible_child_name("remarks");
        // A conversation is not kept: a new document starts without one.
        self.chat.clear();
    }

    /// The selection as the chat quotes it: its Typst, and where it is
    /// (the section, the top-level block's Typst).
    fn selection_quote(&self) -> Option<crate::chat::Quote> {
        let source = self.editor.selection_source()?;
        let doc = self.editor.document();
        let top = doc.top_block(self.editor.caret().block)?;
        let index = doc.blocks.iter().position(|b| b.id == top)?;
        let block = &doc.blocks[index];
        let heading = doc.blocks[..index]
            .iter()
            .rev()
            .find(|b| matches!(b.kind, model::Block::Heading { .. }))
            .map(|h| {
                let (typst, _) = typst_bridge::serialize_block(h);
                typst.trim_start_matches(['=', ' ']).trim().to_string()
            });
        let label = match &block.kind {
            model::Block::Env { name, .. } => name.as_str(),
            _ => block.label(),
        };
        let (block_source, _) = typst_bridge::serialize_block(block);
        let place = crate::chat::place(heading.as_deref(), label, block_source.trim());
        Some(crate::chat::Quote { place, source })
    }

    /// Ask Claude: the chat on the right, the input box focused.
    fn ask_claude(&self) {
        self.split_remarks.set_show_sidebar(true);
        self.right_pages.set_visible_child_name("chat");
        self.chat.focus_input();
    }

    /// Show the remarks in the right sidebar, the caret's selected.
    fn update_remarks(&self) {
        let shown: Vec<Shown> = {
            let doc = self.editor.document();
            doc.remarks_in_order()
                .into_iter()
                .map(|r| Shown {
                    remark: r.clone(),
                    detached: doc.block(r.anchor.block()).is_none(),
                })
                .collect()
        };
        let header = self.editor.header();
        self.remarks
            .update(shown, &self.editor.remarks_at_caret(), &header);
        self.refresh_quotes();
    }

    /// The remark cards' quotations, after the paper or a remark changed.
    fn refresh_quotes(&self) {
        self.remarks.refresh_quotes(
            |id| self.editor.remark_quote_key(id),
            |id| self.editor.remark_quote(id),
        );
        self.remarks.refresh_labels(|| self.editor.outside_labels());
    }

    /// Show the scratch files in the sidebar, with whether they are used.
    fn update_scratch(&self) {
        let paths: Vec<String> = {
            let doc = self.editor.document();
            doc.files
                .iter()
                .filter(|(p, _)| model::is_scratch(p))
                .map(|(p, _)| p.to_string())
                .collect()
        };
        let files: Vec<(String, bool)> = paths
            .into_iter()
            .map(|p| {
                let used = self.editor.file_used(&p);
                (p, used)
            })
            .collect();
        self.versions.update_scratch(&files);
    }

    /// Show the history's versions in the sidebar.
    fn update_versions(&self) {
        let versions = self
            .history
            .borrow()
            .as_ref()
            .and_then(|h| h.versions().ok())
            .unwrap_or_default();
        self.versions.update(&versions);
    }

    /// Name a version (`None`: the current one, saved first) with a name
    /// asked for; the file is saved, so the name is kept.
    async fn name_version(self: Rc<Self>, id: Option<git2::Oid>) {
        // An old version's window: name that version, in its document.
        let old = self.origin.borrow().clone();
        if let Some(old) = old {
            if let Some(origin) = old.window.upgrade() {
                Box::pin(origin.name_version(Some(old.id))).await;
            }
            return;
        }
        let Some(name) = self.ask_version_name().await else {
            return;
        };
        if id.is_none() && !self.save().await {
            return;
        }
        let head = |w: &Self| {
            let history = w.history.borrow();
            history.as_ref()?.versions().ok()?.first().map(|v| v.id)
        };
        let Some(id) = id.or_else(|| head(&self)) else {
            return;
        };
        let tagged = match self.history.borrow().as_ref() {
            Some(history) => history.tag(id, &name, &files::author()),
            None => Err("no version history".into()),
        };
        match tagged {
            Ok(()) => _ = self.save().await,
            Err(e) => self.toast(&format!("Could not name the version: {e}")),
        }
        self.update_versions();
    }

    /// Put a new remark on the selection, or on the block the caret is in,
    /// and type its text in its card in the sidebar.
    fn add_remark(&self) {
        let Some(id) = self.editor.add_remark("") else {
            self.toast("Remarks cannot be added to an old version");
            return;
        };
        self.split_remarks.set_show_sidebar(true);
        self.right_pages.set_visible_child_name("remarks");
        self.update_remarks();
        self.remarks.edit(id);
        // The selection now belongs to the remark, not to the chat.
        self.chat.set_quote(None);
    }

    /// Ask for a version's name ("arXiv submission").
    async fn ask_version_name(&self) -> Option<String> {
        let entry = gtk::Entry::builder()
            .placeholder_text("e.g. arXiv submission")
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::new(Some("Name This Version"), None);
        dialog.set_extra_child(Some(&entry));
        dialog.add_responses(&[("cancel", "_Cancel"), ("name", "_Name")]);
        dialog.set_response_appearance("name", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("name"));
        dialog.set_close_response("cancel");
        let answer = dialog.choose_future(Some(&self.window)).await;
        let name = entry.text().trim().to_string();
        (answer == "name" && !name.is_empty()).then_some(name)
    }

    /// Unsaved changes; an imported `.typ` counts until it is saved as `.sem`.
    fn is_dirty(&self) -> bool {
        if self.imported.borrow().is_some() {
            return true;
        }
        let doc = self.editor.document();
        let saved = self.saved.borrow();
        doc.header != saved.0
            || doc.blocks != saved.1
            || doc.files != saved.2
            || doc.remarks != saved.3
    }

    /// File name without extension, for titles and export names.
    fn stem(&self) -> String {
        self.file
            .borrow()
            .as_ref()
            .and_then(|f| f.basename())
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .or_else(|| self.imported.borrow().clone())
            .unwrap_or_else(|| "Untitled".into())
    }

    fn update_title(&self) {
        if let Some(old) = self.origin.borrow().as_ref() {
            let title = format!("{} — {} (read-only)", old.stem, old.label);
            self.title.set_title(&title);
            self.window
                .set_title(Some(&format!("{title} — Semantic Editor")));
            return;
        }
        let name = match self.file.borrow().as_ref().and_then(|f| f.basename()) {
            Some(path) => path.to_string_lossy().into_owned(),
            None => match self.imported.borrow().as_ref() {
                Some(stem) => format!("{stem}.typ (imported)"),
                None => "Untitled".into(),
            },
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

    /// Move to the next (or previous) problem and say what it is.
    fn go_to_problem(&self, forward: bool) {
        let message = match self.editor.go_to_problem(forward) {
            Some((k, n, problem)) => format!("Problem {k} of {n}: {problem}"),
            None => "No problems found".to_string(),
        };
        if let Some(old) = self.problem_toast.take() {
            old.dismiss();
        }
        let toast = adw::Toast::builder()
            .title(&message)
            .use_markup(false)
            .build();
        self.toasts.add_toast(toast.clone());
        *self.problem_toast.borrow_mut() = Some(toast);
        self.editor.grab_focus();
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }
}

/// What a save stores, to tell unsaved changes.
fn saved_state(doc: &Document) -> (String, Vec<BlockNode>, Files, Vec<Remark>) {
    (
        doc.header.clone(),
        doc.blocks.clone(),
        doc.files.clone(),
        doc.remarks.clone(),
    )
}

/// An empty document with the default header.
fn new_document() -> Document {
    let mut doc = Document::new();
    doc.header = typst_bridge::DEFAULT_HEADER.into();
    doc
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some("_Save"), Some("win.save"));
    file.append(Some("Save _As…"), Some("win.save-as"));
    menu.append_section(None, &file);
    let export = gio::Menu::new();
    export.append(Some("Export _PDF…"), Some("win.export-pdf"));
    export.append(Some("Export _Typst Project…"), Some("win.export-typ"));
    menu.append_section(None, &export);
    let zoom = gio::Menu::new();
    zoom.append(Some("Zoom _In"), Some("win.zoom-in"));
    zoom.append(Some("Zoom _Out"), Some("win.zoom-out"));
    zoom.append(Some("_Reset Zoom"), Some("win.zoom-reset"));
    zoom.append(Some("Show _Code"), Some("win.show-code"));
    zoom.append(Some("Next _Problem"), Some("win.next-problem"));
    menu.append_section(None, &zoom);
    let insert = gio::Menu::new();
    insert.append(Some("Document _Header…"), Some("win.edit-header"));
    insert.append(Some("Document _Title…"), Some("win.insert-title"));
    insert.append(Some("Insert _Environment"), Some("win.insert-env"));
    insert.append(Some("Insert _Picture…"), Some("win.insert-picture"));
    insert.append(
        Some("Insert _Scratch Picture…"),
        Some("win.insert-scratch-picture"),
    );
    insert.append(Some("Insert _Diagram"), Some("win.insert-diagram"));
    insert.append(Some("Insert _Raw Typst"), Some("win.insert-raw"));
    insert.append(
        Some("Insert _Bibliography"),
        Some("win.insert-bibliography"),
    );
    menu.append_section(None, &insert);
    let remarks = gio::Menu::new();
    remarks.append(Some("Add _Remark"), Some("win.add-remark"));
    remarks.append(Some("Ask _Claude…"), Some("win.ask-claude"));
    menu.append_section(None, &remarks);
    let other = gio::Menu::new();
    other.append(Some("Open S_ample Document"), Some("win.open-sample"));
    menu.append_section(None, &other);
    menu
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
