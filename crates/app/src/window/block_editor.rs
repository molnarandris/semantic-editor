//! Source editor for diagram and raw Typst blocks: a dialog with the source
//! on the left and the block's live render on the right. Edits apply to the
//! model after a short pause; the editor widget's worker renders them and
//! the preview shows the same texture as the page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use model::{Block, NodeId};
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
    /// The block being edited.
    block: NodeId,
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

    /// Open the dialog for a raw/diagram block's source.
    pub(super) fn open_block_editor(self: &Rc<Self>, block: NodeId) {
        self.close_block_editor();
        let Some(source) = self.editor.block_source(block) else {
            return;
        };
        let title = match self.editor.document().block(block).map(|b| &b.kind) {
            Some(Block::Diagram(_)) => "Diagram",
            _ => "Raw Typst",
        };
        let subtitle = match self.editor.document().block(block).map(|b| &b.kind) {
            Some(Block::Diagram(_)) => "CeTZ drawing code",
            _ => "Typst markup",
        };
        let ed = Rc::new(build(title, subtitle, block));
        ed.set_text(&source);
        *self.block_editor.borrow_mut() = Some(ed.clone());
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
        if !is_current {
            return;
        }
        self.apply_block_edit();
        self.block_editor.borrow_mut().take();
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
        self.editor.set_block_source(ed.block, &ed.text());
    }

    /// After a command: close the dialog if its block is gone (undo, a new
    /// document), and pick up source changes made elsewhere (undo/redo).
    pub(super) fn sync_block_editor(&self) {
        let Some(ed) = self.current_block_editor() else {
            return;
        };
        match self.editor.block_source(ed.block) {
            None => {
                ed.cancel_debounce();
                self.block_editor.borrow_mut().take();
                ed.dialog.close();
                self.editor.grab_focus();
            }
            Some(source)
                if ed.debounce.borrow().is_none() && !ed.loading.get() && ed.text() != source =>
            {
                ed.set_text(&source);
            }
            Some(_) => {}
        }
    }

    /// A render or error arrived; update the preview if it is our block.
    pub(super) fn block_rendered(&self, block: NodeId) {
        if self
            .current_block_editor()
            .is_some_and(|ed| ed.block == block)
        {
            self.refresh_block_preview();
        }
    }

    fn refresh_block_preview(&self) {
        let Some(ed) = self.current_block_editor() else {
            return;
        };
        ed.preview.set_texture(self.editor.block_texture(ed.block));
        let error = self.editor.block_error(ed.block);
        ed.error.set_visible(error.is_some());
        ed.error.set_label(error.as_deref().unwrap_or(""));
    }
}

/// The dialog: header with title and Done, source | preview, error below
/// the preview.
fn build(title: &str, subtitle: &str, block: NodeId) -> BlockEditor {
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
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(6);
    let source = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .vexpand(true)
        .build();

    let preview = Preview::default();
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
        .position(520)
        .build();

    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(&paned));
    let dialog = adw::Dialog::builder()
        .content_width(1040)
        .content_height(640)
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
        block,
        loading: Cell::new(false),
        debounce: RefCell::new(None),
    }
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
    //! Shows a block texture at its page size (same zoom as the page), shrunk
    //! to fit and centred.

    use std::cell::{Cell, RefCell};

    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{gdk, glib, graphene, gsk};

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct Preview {
            pub texture: RefCell<Option<gdk::Texture>>,
            /// Logical pixels per texture pixel.
            pub scale: Cell<f64>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Preview {
            const NAME: &'static str = "SemBlockPreview";
            type Type = super::Preview;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Preview {}

        impl WidgetImpl for Preview {
            fn measure(
                &self,
                orientation: gtk::Orientation,
                _for_size: i32,
            ) -> (i32, i32, i32, i32) {
                let natural = self.texture.borrow().as_ref().map_or(0, |t| {
                    let px = match orientation {
                        gtk::Orientation::Horizontal => t.width(),
                        _ => t.height(),
                    };
                    (px as f64 * self.scale.get()).ceil() as i32
                });
                (0, natural, -1, -1)
            }

            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let Some(texture) = self.texture.borrow().clone() else {
                    return;
                };
                let widget = self.obj();
                let (aw, ah) = (widget.width() as f64, widget.height() as f64);
                let w = texture.width() as f64 * self.scale.get();
                let h = texture.height() as f64 * self.scale.get();
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                let fit = (aw / w).min(ah / h).min(1.0);
                let (w, h) = (w * fit, h * fit);
                let rect = graphene::Rect::new(
                    ((aw - w) / 2.0).round() as f32,
                    ((ah - h) / 2.0).round() as f32,
                    w as f32,
                    h as f32,
                );
                let filter = if fit < 1.0 {
                    gsk::ScalingFilter::Trilinear
                } else {
                    gsk::ScalingFilter::Linear
                };
                snapshot.append_scaled_texture(&texture, filter, &rect);
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
        /// Show `texture` with `scale` logical pixels per texture pixel.
        pub fn set_texture(&self, texture: Option<(gdk::Texture, f64)>) {
            let imp = self.imp();
            let (texture, scale) = texture.map_or((None, 1.0), |(t, s)| (Some(t), s));
            imp.texture.replace(texture);
            imp.scale.set(scale);
            self.queue_resize();
        }
    }
}
