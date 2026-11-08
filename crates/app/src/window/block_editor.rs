//! Source editor for diagram and raw Typst blocks and for the document
//! header: a dialog with the source on the left and a live render on the
//! right (the block, or a specimen of text styled by the header). Edits
//! apply to the model after a short pause; the editor widget's worker
//! renders the page and, cropped and at the preview's own scale, the
//! preview.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use editor_widget::PreviewTarget;
use gtk::glib;
use model::Block;
use sourceview5::prelude::*;

use super::Window;
use preview::Preview;

/// Pause after the last keystroke before a source edit is applied.
const SOURCE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

pub(super) struct BlockEditor {
    dialog: adw::Dialog,
    view: sourceview5::View,
    preview: Preview,
    error: gtk::Label,
    /// The block being edited, or the header.
    target: PreviewTarget,
    /// Set while the buffer is filled from the model (not a user edit).
    loading: Cell<bool>,
    /// Pending debounced apply of the buffer to the model.
    debounce: RefCell<Option<glib::SourceId>>,
}

impl BlockEditor {
    fn text(&self) -> glib::GString {
        let buffer = self.view.buffer();
        let (start, end) = buffer.bounds();
        buffer.text(&start, &end, false)
    }

    fn set_text(&self, text: &str) {
        self.loading.set(true);
        self.view.buffer().set_text(text);
        self.loading.set(false);
    }

    fn cancel_debounce(&self) {
        if let Some(id) = self.debounce.take() {
            id.remove();
        }
    }
}

impl Window {
    fn current_block_editor(&self) -> Option<Rc<BlockEditor>> {
        self.block_editor.borrow().clone()
    }

    /// The source the dialog edits: a raw/diagram block's, or the header.
    fn source_of(&self, target: PreviewTarget) -> Option<String> {
        match target {
            PreviewTarget::Block(block) => self.editor.block_source(block),
            PreviewTarget::Header => Some(self.editor.header()),
        }
    }

    /// Open the dialog for a raw/diagram block's source or the header.
    pub(super) fn open_block_editor(self: &Rc<Self>, target: PreviewTarget) {
        self.close_block_editor();
        let Some(source) = self.source_of(target) else {
            return;
        };
        let (title, subtitle) = match target {
            PreviewTarget::Header => (
                "Document Header",
                "Set and show rules, imports, definitions",
            ),
            PreviewTarget::Block(block) => {
                match self.editor.document().block(block).map(|b| &b.kind) {
                    Some(Block::Diagram(_)) => ("Diagram", "CeTZ drawing code"),
                    Some(Block::RawTypst(s)) if typst_bridge::is_code(s) => {
                        ("Code", "Definitions and rules for the blocks after it")
                    }
                    _ => ("Raw Typst", "Typst markup"),
                }
            }
        };
        let ed = Rc::new(build(title, subtitle, target));
        ed.set_text(&source);
        *self.block_editor.borrow_mut() = Some(ed.clone());

        // Start at the page's scale; the preview asks for its own once laid out.
        let ppp = self.editor.zoom() * self.window.scale_factor() as f64;
        self.editor.set_preview(Some((target, ppp)));
        let editor = self.editor.downgrade();
        ed.preview.connect_wants_scale(move |ppp| {
            if let Some(editor) = editor.upgrade() {
                editor.set_preview(Some((target, ppp)));
            }
        });
        self.refresh_block_preview();

        let weak = Rc::downgrade(self);
        ed.view.buffer().connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.schedule_block_apply();
            }
        });
        let weak = Rc::downgrade(self);
        let weak_ed = Rc::downgrade(&ed);
        ed.dialog.connect_closed(move |_| {
            if let (Some(this), Some(ed)) = (weak.upgrade(), weak_ed.upgrade()) {
                this.block_editor_closed(&ed);
            }
        });
        ed.dialog.present(Some(&self.window));
        ed.view.grab_focus();
    }

    /// Close the dialog (applying pending edits), if open.
    pub(super) fn close_block_editor(&self) {
        if let Some(ed) = self.current_block_editor() {
            ed.dialog.close();
            // `closed` normally runs `block_editor_closed`; make sure the
            // state is cleared even if the dialog was not shown yet.
            self.block_editor_closed(&ed);
        }
    }

    fn block_editor_closed(&self, ed: &Rc<BlockEditor>) {
        let is_current = self
            .block_editor
            .borrow()
            .as_ref()
            .is_some_and(|cur| Rc::ptr_eq(cur, ed));
        if is_current {
            self.apply_block_edit();
            self.forget_block_editor();
        }
    }

    /// Drop the dialog state (no apply) and stop its preview renders.
    fn forget_block_editor(&self) {
        if let Some(ed) = self.block_editor.borrow_mut().take() {
            ed.cancel_debounce();
        }
        self.editor.set_preview(None);
        self.editor.grab_focus();
    }

    fn schedule_block_apply(self: &Rc<Self>) {
        let Some(ed) = self.current_block_editor() else {
            return;
        };
        if ed.loading.get() {
            return;
        }
        ed.cancel_debounce();
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(SOURCE_DEBOUNCE, move || {
            if let Some(this) = weak.upgrade() {
                if let Some(ed) = this.current_block_editor() {
                    ed.debounce.take();
                }
                this.apply_block_edit();
            }
        });
        *ed.debounce.borrow_mut() = Some(id);
    }

    /// Put the dialog's buffer into the model now (cancels the debounce).
    pub(super) fn apply_block_edit(&self) {
        let Some(ed) = self.current_block_editor() else {
            return;
        };
        ed.cancel_debounce();
        match ed.target {
            PreviewTarget::Block(block) => self.editor.set_block_source(block, &ed.text()),
            PreviewTarget::Header => self.editor.set_header(&ed.text()),
        }
    }

    /// After a command: close the dialog if its block is gone (undo, a new
    /// document), and pick up source changes made elsewhere (undo/redo).
    pub(super) fn sync_block_editor(&self) {
        let Some(ed) = self.current_block_editor() else {
            return;
        };
        match self.source_of(ed.target) {
            None => {
                self.forget_block_editor();
                ed.dialog.close();
            }
            Some(source)
                if ed.debounce.borrow().is_none() && !ed.loading.get() && ed.text() != source =>
            {
                ed.set_text(&source);
            }
            Some(_) => {}
        }
    }

    /// The zoomed preview render or its error arrived.
    pub(super) fn refresh_block_preview(&self) {
        let Some(ed) = self.current_block_editor() else {
            return;
        };
        ed.preview.set_texture(self.editor.preview_texture());
        let error = self.editor.preview_error();
        ed.error.set_visible(error.is_some());
        ed.error.set_label(error.as_deref().unwrap_or(""));
    }
}

/// The dialog: header with title and Done, source | preview, error below
/// the preview.
fn build(title: &str, subtitle: &str, target: PreviewTarget) -> BlockEditor {
    let done = gtk::Button::builder()
        .label("Done")
        .css_classes(["suggested-action"])
        .tooltip_text("Back to the Page (Esc)")
        .build();
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(title, subtitle))
        .show_end_title_buttons(false)
        .build();
    header.pack_end(&done);

    let buffer = sourceview5::Buffer::new(None);
    follow_style_scheme(&buffer);
    let view = sourceview5::View::with_buffer(&buffer);
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_auto_indent(true);
    view.set_tab_width(2);
    view.set_indent_width(2);
    view.set_insert_spaces_instead_of_tabs(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(6);
    hanging_indent(&view);
    let source = gtk::ScrolledWindow::builder()
        .child(&view)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .build();

    let preview = Preview::default();
    preview.set_hexpand(true);
    preview.set_vexpand(true);
    preview.set_margin_start(12);
    preview.set_margin_end(12);
    preview.set_margin_top(12);
    preview.set_margin_bottom(12);
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .visible(false)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .css_classes(["error", "caption"])
        .build();
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.append(&preview);
    right.append(&error);
    right.add_css_class("view");

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&source)
        .end_child(&right)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .position(440)
        .build();

    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(&paned));
    let dialog = adw::Dialog::builder()
        .content_width(1100)
        .content_height(720)
        .child(&content)
        .build();
    let weak = dialog.downgrade();
    done.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });

    BlockEditor {
        dialog,
        view,
        preview,
        error,
        target,
        loading: Cell::new(false),
        debounce: RefCell::new(None),
    }
}

/// Wrapped lines start under the first non-blank character of their line:
/// each line gets a tag with the negative of its indentation as `indent`,
/// which GTK applies as a hanging indent to the continuation lines.
fn hanging_indent(view: &sourceview5::View) {
    let update = {
        let view = view.downgrade();
        move || {
            let Some(view) = view.upgrade() else { return };
            let buffer = view.buffer();
            let table = buffer.tag_table();
            let (start, end) = buffer.bounds();
            let mut names = vec![];
            table.foreach(|tag| {
                if let Some(name) = tag.name().filter(|n| n.starts_with("indent-")) {
                    names.push(name);
                }
            });
            for name in names {
                buffer.remove_tag_by_name(&name, &start, &end);
            }

            let layout = view.create_pango_layout(Some(" "));
            let space = layout.pixel_size().0;
            let tab = view.tab_width() as i32;
            for line in 0..buffer.line_count() {
                let Some(a) = buffer.iter_at_line(line) else {
                    continue;
                };
                let mut columns = 0;
                let mut b = a;
                while !b.ends_line() {
                    match b.char() {
                        ' ' => columns += 1,
                        '\t' => columns += tab - columns % tab,
                        _ => break,
                    }
                    b.forward_char();
                }
                if columns == 0 {
                    continue;
                }
                let name = format!("indent-{columns}");
                if table.lookup(&name).is_none() {
                    let width = columns * space;
                    let tag = gtk::TextTag::builder().name(&name).indent(-width).build();
                    table.add(&tag);
                }
                let mut line_end = a;
                if !line_end.ends_line() {
                    line_end.forward_to_line_end();
                }
                buffer.apply_tag_by_name(&name, &a, &line_end);
            }
        }
    };
    update();
    view.buffer().connect_changed(move |_| update());
}

/// Light or dark source colours, following the system style.
fn follow_style_scheme(buffer: &sourceview5::Buffer) {
    let style = adw::StyleManager::default();
    let set_scheme = {
        let buffer = buffer.downgrade();
        move |style: &adw::StyleManager| {
            let Some(buffer) = buffer.upgrade() else {
                return;
            };
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
    let handler = style.connect_dark_notify(set_scheme);
    // The style manager outlives every dialog: disconnect with the buffer.
    let style = style.downgrade();
    let handler = RefCell::new(Some(handler));
    buffer.add_weak_ref_notify_local(move || {
        if let (Some(style), Some(handler)) = (style.upgrade(), handler.take()) {
            style.disconnect(handler);
        }
    });
}

mod preview {
    //! Shows the preview render scaled to fill the widget (keeping its
    //! aspect ratio), centred. When the scale it is drawn at differs from
    //! the one it was rendered at, it asks for a new render at that scale.

    use std::cell::{Cell, RefCell};

    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{gdk, glib, graphene};

    /// Re-render when the drawn scale is off by more than this fraction.
    const RESCALE_TOLERANCE: f64 = 0.04;

    type ScaleCallback = Box<dyn Fn(f64)>;

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct Preview {
            pub texture: RefCell<Option<gdk::Texture>>,
            /// Device pixels per point the texture was rendered at.
            pub pixel_per_pt: Cell<f64>,
            /// Last scale asked for, to ask only once.
            pub asked: Cell<f64>,
            pub wants_scale: RefCell<Option<ScaleCallback>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Preview {
            const NAME: &'static str = "SemBlockPreview";
            type Type = super::Preview;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Preview {}

        impl WidgetImpl for Preview {
            fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
                self.parent_size_allocate(width, height, baseline);
                self.ask_for_scale();
            }

            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let Some(texture) = self.texture.borrow().clone() else {
                    return;
                };
                let Some(fit) = self.fit() else { return };
                let widget = self.obj();
                let sf = widget.scale_factor() as f64;
                let ppp = self.pixel_per_pt.get();
                // Logical size; one texture pixel per device pixel when the
                // render is at the drawn scale, so it stays sharp.
                let (w, h) = if (fit * sf / ppp - 1.0).abs() <= RESCALE_TOLERANCE {
                    (texture.width() as f64 / sf, texture.height() as f64 / sf)
                } else {
                    (
                        texture.width() as f64 / ppp * fit,
                        texture.height() as f64 / ppp * fit,
                    )
                };
                let (aw, ah) = (widget.width() as f64, widget.height() as f64);
                let rect = graphene::Rect::new(
                    ((aw - w) / 2.0 * sf).round() as f32 / sf as f32,
                    ((ah - h) / 2.0 * sf).round() as f32 / sf as f32,
                    w as f32,
                    h as f32,
                );
                snapshot.append_texture(&texture, &rect);
            }
        }

        impl Preview {
            /// Logical pixels per point that make the render fill the widget.
            pub fn fit(&self) -> Option<f64> {
                let texture = self.texture.borrow();
                let texture = texture.as_ref()?;
                let ppp = self.pixel_per_pt.get();
                let (w_pt, h_pt) = (texture.width() as f64 / ppp, texture.height() as f64 / ppp);
                let widget = self.obj();
                let (aw, ah) = (widget.width() as f64, widget.height() as f64);
                if w_pt <= 0.0 || h_pt <= 0.0 || aw <= 0.0 || ah <= 0.0 {
                    return None;
                }
                Some((aw / w_pt).min(ah / h_pt))
            }

            pub fn ask_for_scale(&self) {
                let Some(fit) = self.fit() else { return };
                let wanted = fit * self.obj().scale_factor() as f64;
                let near = |a: f64, b: f64| (a / b - 1.0).abs() <= RESCALE_TOLERANCE;
                if near(wanted, self.pixel_per_pt.get()) || near(wanted, self.asked.get()) {
                    return;
                }
                self.asked.set(wanted);
                if let Some(f) = self.wants_scale.borrow().as_ref() {
                    f(wanted);
                }
            }
        }
    }

    glib::wrapper! {
        pub struct Preview(ObjectSubclass<imp::Preview>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Default for Preview {
        fn default() -> Self {
            glib::Object::new()
        }
    }

    impl Preview {
        /// Show `texture`, rendered at `pixel_per_pt` device pixels per point.
        pub fn set_texture(&self, texture: Option<(gdk::Texture, f64)>) {
            let imp = self.imp();
            let (texture, ppp) = texture.map_or((None, 1.0), |(t, p)| (Some(t), p));
            imp.texture.replace(texture);
            imp.pixel_per_pt.set(ppp);
            imp.ask_for_scale();
            self.queue_draw();
        }

        /// Called with the device pixels per point the preview should be
        /// rendered at to fill the widget.
        pub fn connect_wants_scale(&self, f: impl Fn(f64) + 'static) {
            self.imp().wants_scale.replace(Some(Box::new(f)));
        }
    }
}
