//! The outline pane: the document's headings, the one the caret is under
//! selected. Activating one (click, or Enter) goes to it. Independent of
//! an `#outline()` in the document. Each heading shows as Typst renders it
//! as an outline entry (number, title with its formulas; see
//! `SemEditor::outline_entry`), as plain text until that render arrives.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use editor_widget::SemEditor;
use gtk::gdk;
use model::{Block, Document, Fragment, Inline, InlineNode, NodeId};

/// Indent per heading level of the plain-text stand-in, in px.
const INDENT_PX: i32 = 14;

#[derive(Clone, PartialEq)]
struct Entry {
    id: NodeId,
    level: u8,
    title: String,
}

pub struct Outline {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    /// The headings shown, row by row.
    entries: Rc<RefCell<Vec<Entry>>>,
}

impl Outline {
    pub fn new() -> Self {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar", "outline-pane"])
            .build();
        load_css();
        let empty = gtk::Label::builder()
            .label("No headings")
            .css_classes(["dim-label"])
            .margin_top(12)
            .build();
        list.set_placeholder(Some(&empty));
        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .build();
        Self {
            widget,
            list,
            entries: Rc::default(),
        }
    }

    /// Call `f` with the heading's block when one is activated.
    pub fn connect_activated(&self, f: impl Fn(NodeId) + 'static) {
        let entries = self.entries.clone();
        self.list.connect_row_activated(move |_, row| {
            let id = entries.borrow().get(row.index() as usize).map(|e| e.id);
            if let Some(id) = id {
                f(id);
            }
        });
    }

    /// Show the headings of the editor's document (rebuilding the rows only
    /// if they changed) and select the one the caret is under.
    pub fn update(&self, editor: &SemEditor) {
        let doc = editor.document();
        let at = editor.caret().block;
        let entries = headings(&doc);
        if *self.entries.borrow() != entries {
            while let Some(row) = self.list.row_at_index(0) {
                self.list.remove(&row);
            }
            for e in &entries {
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&content(e, editor.outline_entry(e.id))));
                self.list.append(&row);
            }
            *self.entries.borrow_mut() = entries;
        }
        let here = doc.top_index(at).unwrap_or(0);
        let current = doc.blocks[..=here.min(doc.blocks.len() - 1)]
            .iter()
            .rev()
            .find(|b| matches!(b.kind, Block::Heading { .. }))
            .map(|b| b.id);
        let index = current.and_then(|id| self.entries.borrow().iter().position(|e| e.id == id));
        match index.and_then(|i| self.list.row_at_index(i as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.list.unselect_all(),
        }
    }

    /// Show the new render of heading `id`'s entry.
    pub fn show_render(&self, editor: &SemEditor, id: NodeId) {
        let entries = self.entries.borrow();
        let Some(i) = entries.iter().position(|e| e.id == id) else {
            return;
        };
        if let Some(row) = self.list.row_at_index(i as i32) {
            row.set_child(Some(&content(&entries[i], editor.outline_entry(id))));
        }
    }
}

/// Rows as high as their entry plus a little padding (the sidebar's rows
/// have a minimum height, which spaced one-line entries more than wrapped
/// ones).
fn load_css() {
    thread_local!(static LOADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if LOADED.replace(true) {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".outline-pane > row { min-height: 0; padding-top: 4px; padding-bottom: 4px; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// A row's content: the entry's render, or its plain title.
fn content(e: &Entry, render: Option<(gdk::Texture, f64)>) -> gtk::Widget {
    match render {
        Some((texture, scale)) => {
            let picture = Render::default();
            picture.set(texture, scale, &e.title);
            picture.upcast()
        }
        None => plain_row(e).upcast(),
    }
}

fn plain_row(e: &Entry) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(&e.title)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .margin_start(INDENT_PX * (e.level.max(1) as i32 - 1))
        .tooltip_text(&e.title)
        .build();
    if e.level <= 1 {
        label.add_css_class("heading");
    }
    label
}

/// The top-level headings of `doc`, in order.
fn headings(doc: &Document) -> Vec<Entry> {
    doc.blocks
        .iter()
        .filter_map(|b| match &b.kind {
            Block::Heading { level, inlines } => Some(Entry {
                id: b.id,
                level: *level,
                title: plain(inlines),
            }),
            _ => None,
        })
        .collect()
}

/// A heading's text: its words, formulas as their Typst source, footnotes
/// left out.
fn plain(inlines: &[InlineNode]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match &inline.kind {
            Inline::Text(s) | Inline::Raw(s) => out.push_str(s),
            Inline::Ref(label) => {
                out.push('@');
                out.push_str(label);
            }
            Inline::Call { name, .. } if name == "footnote" => {}
            Inline::Call { body, .. } => out.push_str(&plain(body)),
            Inline::Math(_) => {
                let fragment = Fragment::Inlines(vec![inline.clone()]);
                out.push_str(&typst_bridge::fragment_source(&fragment));
            }
        }
    }
    let title = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        "(untitled)".into()
    } else {
        title
    }
}

/// A texture drawn at its logical size (device pixels / scale), so a
/// render for a HiDPI screen is not shown twice as large.
mod render {
    use std::cell::{Cell, RefCell};

    use gtk::subclass::prelude::*;
    use gtk::{gdk, glib, graphene, prelude::*};

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct Render {
            pub texture: RefCell<Option<gdk::Texture>>,
            /// Device pixels per logical pixel of the texture.
            pub scale: Cell<f64>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Render {
            const NAME: &'static str = "SemOutlineRender";
            type Type = super::Render;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Render {}

        impl WidgetImpl for Render {
            fn measure(
                &self,
                orientation: gtk::Orientation,
                _for_size: i32,
            ) -> (i32, i32, i32, i32) {
                let (w, h) = self.size();
                let size = match orientation {
                    gtk::Orientation::Horizontal => w,
                    _ => h,
                };
                (size, size, -1, -1)
            }

            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let Some(texture) = self.texture.borrow().clone() else {
                    return;
                };
                let (w, h) = self.size();
                snapshot
                    .append_texture(&texture, &graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
            }
        }

        impl Render {
            /// Logical size, in px.
            fn size(&self) -> (i32, i32) {
                let scale = self.scale.get().max(0.1);
                self.texture.borrow().as_ref().map_or((0, 0), |t| {
                    (
                        (t.width() as f64 / scale).ceil() as i32,
                        (t.height() as f64 / scale).ceil() as i32,
                    )
                })
            }
        }
    }

    glib::wrapper! {
        pub struct Render(ObjectSubclass<imp::Render>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Default for Render {
        fn default() -> Self {
            glib::Object::new()
        }
    }

    impl Render {
        /// Show `texture` (`scale` device pixels per logical pixel), with
        /// `title` as its tooltip.
        pub fn set(&self, texture: gdk::Texture, scale: f64, title: &str) {
            self.imp().texture.replace(Some(texture));
            self.imp().scale.set(scale);
            self.set_tooltip_text(Some(title));
            self.set_halign(gtk::Align::Start);
            self.set_valign(gtk::Align::Center);
            self.queue_resize();
        }
    }
}

use render::Render;

#[cfg(test)]
mod tests {
    use super::*;
    use model::Editor;

    #[test]
    fn heading_titles() {
        let mut e = Editor::new(Document::new());
        e.insert_text("= The $x$ map#footnote[n]");
        e.enter();
        e.insert_text("text");
        e.enter();
        e.insert_text("== *Bold*  part");
        let got: Vec<(u8, String)> = headings(e.doc())
            .into_iter()
            .map(|h| (h.level, h.title))
            .collect();
        assert_eq!(
            got,
            [(1, "The $x$ map".to_string()), (2, "Bold part".to_string())]
        );
    }
}
