//! The outline pane: the document's headings, indented by level, the one
//! the caret is under selected. Activating one (click, or Enter) goes to
//! it. Independent of an `#outline()` in the document.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use model::{Block, Document, Fragment, Inline, InlineNode, NodeId};

/// Indent per heading level, in px.
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
            .css_classes(["navigation-sidebar"])
            .build();
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

    /// Show the headings of `doc` (rebuilding the rows only if they
    /// changed) and select the one the caret's block `at` is under.
    pub fn update(&self, doc: &Document, at: NodeId) {
        let entries = headings(doc);
        if *self.entries.borrow() != entries {
            while let Some(row) = self.list.row_at_index(0) {
                self.list.remove(&row);
            }
            for e in &entries {
                self.list.append(&row(e));
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
}

fn row(e: &Entry) -> gtk::ListBoxRow {
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
    gtk::ListBoxRow::builder().child(&label).build()
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
