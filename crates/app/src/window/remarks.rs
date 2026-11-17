//! The Remarks pane on the right: a card per remark in the order of the
//! text, on old-paper yellow, the ones the caret is in selected. The text
//! is edited right in the card, by an editor like the paper's (the
//! paper's header in force), and kept as Typst. Clicking a card selects
//! its text; Resolve on each card; resolved remarks below, dimmed, with
//! Reopen and Delete.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use editor_widget::{Outside, SemEditor};
use model::{Document, Fragment, Remark};

/// The remark editors' zoom: about the size of the interface's text.
const CARD_ZOOM: f64 = editor_widget::DEFAULT_ZOOM * 0.75;

/// What a card asks the window to do.
#[derive(Clone, Debug)]
pub enum RemarkAction {
    /// Select its text in the paper and go there.
    Select(u64),
    /// Show its text in the paper, the focus staying in the card.
    Show(u64),
    SetText(u64, String),
    Resolve(u64, bool),
    Delete(u64),
    /// Go to the paper's block with this label (Ctrl+click on a reference
    /// to it in a card).
    FollowRef(String),
}

type Callback = Rc<RefCell<Option<Box<dyn Fn(RemarkAction)>>>>;

/// A remark as shown: whether its text is still there.
#[derive(Clone, PartialEq)]
pub struct Shown {
    pub remark: Remark,
    pub detached: bool,
}

pub struct RemarksPane {
    pub widget: gtk::Box,
    scroller: gtk::ScrolledWindow,
    open: gtk::ListBox,
    resolved: gtk::ListBox,
    resolved_title: gtk::Label,
    empty: gtk::Label,
    /// The remarks the cards show, to rebuild only on a change (a card's
    /// own edits are written here first, so they do not rebuild it).
    shown: Rc<RefCell<Vec<Shown>>>,
    /// The paper's header, for the remarks' editors.
    header: RefCell<String>,
    /// The paper's labels, which `@label` in a card's text refers to.
    labels: RefCell<Vec<Outside>>,
    /// The cards, open ones first in list order.
    cards: RefCell<Vec<Card>>,
    read_only: Cell<bool>,
    on_action: Callback,
}

impl RemarksPane {
    pub fn new() -> Self {
        load_css();
        let list = || {
            gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["remark-list"])
                .build()
        };
        let (open, resolved) = (list(), list());
        open.set_selection_mode(gtk::SelectionMode::Multiple);
        let resolved_title = gtk::Label::builder()
            .label("Resolved")
            .xalign(0.0)
            .css_classes(["heading"])
            .margin_start(12)
            .margin_top(12)
            .margin_bottom(4)
            .build();
        let empty = gtk::Label::builder()
            .label("No remarks.\nSelect text and press Ctrl+Alt+M\nto add one.")
            .css_classes(["dim-label"])
            .justify(gtk::Justification::Center)
            .margin_top(24)
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&open);
        column.append(&empty);
        column.append(&resolved_title);
        column.append(&resolved);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&column)
            .build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&scroller);

        let on_action: Callback = Rc::default();
        let action = on_action.clone();
        open.connect_row_activated(move |_, row| {
            // A click in the card's text edits it: the press gave the text
            // the focus, the release must not take it back to the paper.
            let in_text = row
                .root()
                .and_then(|r| r.focus())
                .is_some_and(|f| f.is::<SemEditor>() && f.is_ancestor(row));
            if in_text {
                return;
            }
            if let Some(f) = action.borrow().as_ref() {
                f(RemarkAction::Select(row_id(row)));
            }
        });
        let this = Self {
            widget,
            scroller,
            open,
            resolved,
            resolved_title,
            empty,
            shown: Rc::default(),
            header: RefCell::default(),
            labels: RefCell::default(),
            cards: RefCell::default(),
            read_only: Cell::new(false),
            on_action,
        };
        this.rebuild(&[]);
        this
    }

    /// Call `f` with what a card asks for.
    pub fn connect_action(&self, f: impl Fn(RemarkAction) + 'static) {
        *self.on_action.borrow_mut() = Some(Box::new(f));
    }

    /// An old version: cards without buttons, texts not editable.
    pub fn set_read_only(&self, read_only: bool) {
        self.read_only.set(read_only);
        let shown = self.shown.take();
        self.rebuild(&shown);
        *self.shown.borrow_mut() = shown;
    }

    /// Show `remarks` (in text order), written with the paper's `header` in
    /// force, and select the cards of `current` (the remarks the caret is
    /// in), scrolled into view.
    pub fn update(&self, remarks: Vec<Shown>, current: &[u64], header: &str) {
        let new_header = *self.header.borrow() != header;
        if new_header {
            *self.header.borrow_mut() = header.to_string();
        }
        if new_header || *self.shown.borrow() != remarks {
            self.rebuild(&remarks);
            *self.shown.borrow_mut() = remarks;
        }
        let cards = self.cards.borrow();
        let mut first = None;
        let open = cards.iter().filter(|c| !c.resolved);
        for (i, Card { id, .. }) in open.enumerate() {
            let Some(row) = self.open.row_at_index(i as i32) else {
                continue;
            };
            if current.contains(id) {
                self.open.select_row(Some(&row));
                first.get_or_insert(row);
            } else {
                self.open.unselect_row(&row);
            }
        }
        if let Some(row) = first {
            self.scroll_to(&row);
        }
    }

    /// Put the keyboard focus in remark `id`'s text (a new remark).
    pub fn edit(&self, id: u64) {
        let cards = self.cards.borrow();
        if let Some(card) = cards.iter().find(|c| c.id == id) {
            card.text.grab_focus();
        }
    }

    /// Let `@label` in the cards' texts refer to the paper's labels
    /// (`labels`, asked for only when there are cards).
    pub fn refresh_labels(&self, labels: impl FnOnce() -> Vec<Outside>) {
        let cards = self.cards.borrow();
        if cards.is_empty() {
            return;
        }
        let labels = labels();
        if *self.labels.borrow() == labels {
            return;
        }
        for card in cards.iter() {
            card.text.set_outside_labels(labels.clone());
        }
        *self.labels.borrow_mut() = labels;
    }

    /// Show each card's quotation: `key` tells what it depends on, `quote`
    /// makes it (only when the key changed: the paper or the range did).
    pub fn refresh_quotes(
        &self,
        key: impl Fn(u64) -> Option<Vec<u64>>,
        quote: impl Fn(u64) -> Option<(gtk::gdk::Texture, f64)>,
    ) {
        for card in self.cards.borrow().iter() {
            let new = key(card.id);
            if *card.key.borrow() == new {
                continue;
            }
            let texture = new.as_ref().and_then(|_| quote(card.id));
            let shown = texture.map(|(t, ppp)| scaled::Scaled::new(t, ppp, CARD_ZOOM));
            card.quote.set_paintable(shown.as_ref());
            card.quote.set_visible(shown.is_some());
            *card.key.borrow_mut() = new;
        }
    }

    /// Scroll so that `row` is in view.
    fn scroll_to(&self, row: &gtk::ListBoxRow) {
        let Some(child) = self.scroller.child() else {
            return;
        };
        let Some(bounds) = row.compute_bounds(&child) else {
            return;
        };
        let adj = self.scroller.vadjustment();
        let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
        if top < adj.value() {
            adj.set_value(top);
        } else if bottom > adj.value() + adj.page_size() {
            adj.set_value((bottom - adj.page_size()).min(top));
        }
    }

    fn rebuild(&self, remarks: &[Shown]) {
        self.open.remove_all();
        self.resolved.remove_all();
        let mut cards = vec![];
        for shown in remarks.iter().filter(|s| !s.remark.resolved) {
            let (row, card) = self.card(shown);
            self.open.append(&row);
            cards.push(card);
        }
        self.empty.set_visible(cards.is_empty());
        let resolved_any = remarks.iter().any(|s| s.remark.resolved);
        for shown in remarks.iter().filter(|s| s.remark.resolved) {
            let (row, card) = self.card(shown);
            self.resolved.append(&row);
            cards.push(card);
        }
        self.resolved_title.set_visible(resolved_any);
        self.resolved.set_visible(resolved_any);
        *self.cards.borrow_mut() = cards;
    }

    /// The card of a remark: the quotation of what it is about (read-only,
    /// shown once rendered), its text (edited in place), buttons.
    fn card(&self, shown: &Shown) -> (gtk::ListBoxRow, Card) {
        let r = &shown.remark;
        let id = r.id;
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["remark-card"])
            .build();
        let quote = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::ScaleDown)
            .halign(gtk::Align::Start)
            .visible(false)
            .css_classes(["remark-quote"])
            .build();
        column.append(&quote);
        if shown.detached {
            column.append(&dim("Its text was deleted."));
        }
        let editable = !self.read_only.get() && !r.resolved;
        let text = SemEditor::new();
        text.set_fit_content(true);
        text.set_zoom(CARD_ZOOM);
        text.set_hexpand(true);
        let mut doc = typst_bridge::import_typ(&r.text);
        if doc.blocks.is_empty() {
            let p = doc.paragraph(vec![]);
            doc.blocks.push(p);
        }
        doc.header = self.header.borrow().clone();
        text.set_document(doc);
        text.set_read_only(!editable);
        text.set_outside_labels(self.labels.borrow().clone());
        let on_action = self.on_action.clone();
        text.connect_follow_outside(move |_, label| {
            if let Some(f) = on_action.borrow().as_ref() {
                f(RemarkAction::FollowRef(label.to_string()));
            }
        });
        if editable {
            self.connect_text(&text, id);
        }
        column.append(&text);
        if !self.read_only.get() {
            column.append(&self.buttons(r));
        }
        let row = gtk::ListBoxRow::builder()
            .child(&column)
            .name(id.to_string())
            .build();
        if r.resolved {
            row.add_css_class("dim-label");
        }
        let card = Card {
            id,
            resolved: r.resolved,
            text,
            quote,
            key: RefCell::default(),
        };
        (row, card)
    }

    /// A card's text: every change goes to the remark (as Typst); entering
    /// it shows the remark's text in the paper; leaving it empty deletes
    /// the remark.
    fn connect_text(&self, text: &SemEditor, id: u64) {
        let (shown, on_action) = (self.shown.clone(), self.on_action.clone());
        // What the editor holds as Typst: changed only by an edit.
        let last = RefCell::new(source(&text.document()));
        text.connect_caret_moved(move |text| {
            let new = source(&text.document());
            if *last.borrow() == new {
                return;
            }
            *last.borrow_mut() = new.clone();
            if let Some(s) = shown.borrow_mut().iter_mut().find(|s| s.remark.id == id) {
                s.remark.text = new.clone();
            }
            if let Some(f) = on_action.borrow().as_ref() {
                f(RemarkAction::SetText(id, new));
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let on_action = self.on_action.clone();
        focus.connect_enter(move |_| {
            if let Some(f) = on_action.borrow().as_ref() {
                f(RemarkAction::Show(id));
            }
        });
        let on_action = self.on_action.clone();
        let view = text.downgrade();
        focus.connect_leave(move |_| {
            let empty = view
                .upgrade()
                .is_some_and(|v| source(&v.document()).trim().is_empty());
            if empty && let Some(f) = on_action.borrow().as_ref() {
                f(RemarkAction::Delete(id));
            }
        });
        text.add_controller(focus);
    }

    /// Resolve, or Reopen and Delete for a resolved remark.
    fn buttons(&self, r: &Remark) -> gtk::Box {
        let buttons = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::End)
            .build();
        let id = r.id;
        let actions: Vec<(&str, RemarkAction)> = if r.resolved {
            vec![
                ("Delete", RemarkAction::Delete(id)),
                ("Reopen", RemarkAction::Resolve(id, false)),
            ]
        } else {
            vec![("Resolve", RemarkAction::Resolve(id, true))]
        };
        for (label, action) in actions {
            let button = gtk::Button::builder()
                .label(label)
                .css_classes(["flat", "caption"])
                .build();
            let on_action = self.on_action.clone();
            button.connect_clicked(move |_| {
                if let Some(f) = on_action.borrow().as_ref() {
                    f(action.clone());
                }
            });
            buttons.append(&button);
        }
        buttons
    }
}

/// A card's parts that change after it is built.
struct Card {
    id: u64,
    resolved: bool,
    text: SemEditor,
    quote: gtk::Picture,
    /// What its quotation shows (see `refresh_quotes`).
    key: RefCell<Option<Vec<u64>>>,
}

/// A remark's text as Typst: its editor's blocks, without the header.
fn source(doc: &Document) -> String {
    typst_bridge::fragment_source(&Fragment::Blocks(doc.blocks.clone()))
}

/// The remark id a card stands for.
fn row_id(row: &gtk::ListBoxRow) -> u64 {
    row.widget_name().parse().unwrap_or_default()
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build()
}

/// Cards on old-paper yellow with dark text in any theme, as the paper is
/// white in any theme (the quotation and the remark are rendered in black);
/// the cards the caret is in outlined.
fn load_css() {
    thread_local!(static LOADED: Cell<bool> = const { Cell::new(false) });
    if LOADED.replace(true) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".remark-list { background: none; }
         .remark-list > row { padding: 4px 8px; background: none; }
         .remark-card { background: #f8f0d8; color: #2e2a22; border-radius: 6px;
                        padding: 8px; border: 1px solid alpha(#9a7a20, 0.3); }
         .remark-card button { color: #2e2a22; }
         .remark-list > row:selected .remark-card { border-color: alpha(#c08a00, 0.9);
                        box-shadow: 0 0 0 1px alpha(#c08a00, 0.9); }
         .remark-quote { padding-bottom: 6px; margin-bottom: 2px;
                         border-bottom: 1px solid alpha(#9a7a20, 0.3); }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

mod scaled {
    //! A quotation's texture at the size of the card's text: its pixels
    //! (rendered at the paper's pixels per point) count as points at the
    //! card's zoom, so the letters do not change with the paper's zoom or
    //! the screen. The picture shrinks it only if the card is narrower.

    use std::cell::{Cell, OnceCell};

    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{gdk, glib};

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct Scaled {
            pub texture: OnceCell<gdk::Texture>,
            /// Logical pixels per texture pixel.
            pub scale: Cell<f64>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Scaled {
            const NAME: &'static str = "SemScaledQuote";
            type Type = super::Scaled;
            type Interfaces = (gdk::Paintable,);
        }

        impl ObjectImpl for Scaled {}

        impl PaintableImpl for Scaled {
            fn intrinsic_width(&self) -> i32 {
                let w = self.texture.get().map_or(0, |t| t.width());
                (w as f64 * self.scale.get()).round() as i32
            }

            fn intrinsic_height(&self) -> i32 {
                let h = self.texture.get().map_or(0, |t| t.height());
                (h as f64 * self.scale.get()).round() as i32
            }

            fn flags(&self) -> gdk::PaintableFlags {
                gdk::PaintableFlags::STATIC_SIZE | gdk::PaintableFlags::STATIC_CONTENTS
            }

            fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
                if let Some(texture) = self.texture.get() {
                    texture.snapshot(snapshot, width, height);
                }
            }
        }
    }

    glib::wrapper! {
        pub struct Scaled(ObjectSubclass<imp::Scaled>) @implements gdk::Paintable;
    }

    impl Scaled {
        /// `texture` rendered at `pixel_per_pt` device pixels per point,
        /// shown at `zoom` logical pixels per point.
        pub fn new(texture: gdk::Texture, pixel_per_pt: f64, zoom: f64) -> Self {
            let obj: Self = glib::Object::new();
            let _ = obj.imp().texture.set(texture);
            obj.imp().scale.set(zoom / pixel_per_pt);
            obj
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remark's text read into its card's editor and written back is the
    /// same (so showing a remark is not an edit), formulas and markup too.
    #[test]
    fn remark_text_round_trips() {
        for text in [
            "Is this the right word?",
            "Check $A^i$ here, and *this*.",
            "Two paragraphs.\n\nThe second one.",
            "- a list\n- of two",
        ] {
            assert_eq!(source(&typst_bridge::import_typ(text)), text);
        }
        assert_eq!(source(&typst_bridge::import_typ("")), "");
    }
}
