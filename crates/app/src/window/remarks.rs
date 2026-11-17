//! The Remarks pane on the right: a card per remark in the order of the
//! text, on old-paper yellow, the ones the caret is in selected. The text
//! is edited right in the card. Clicking a card selects its text; Resolve
//! on each card; resolved remarks below, dimmed, with Reopen and Delete.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use model::Remark;

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
    /// The open cards' remark ids, in list order, with their text views.
    open_cards: RefCell<Vec<(u64, gtk::TextView)>>,
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
            open_cards: RefCell::default(),
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

    /// Show `remarks` (in text order) and select the cards of `current`
    /// (the remarks the caret is in), scrolled into view.
    pub fn update(&self, remarks: Vec<Shown>, current: &[u64]) {
        if *self.shown.borrow() != remarks {
            self.rebuild(&remarks);
            *self.shown.borrow_mut() = remarks;
        }
        let cards = self.open_cards.borrow();
        let mut first = None;
        for (i, (id, _)) in cards.iter().enumerate() {
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
        let cards = self.open_cards.borrow();
        if let Some((_, text)) = cards.iter().find(|(i, _)| *i == id) {
            text.grab_focus();
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
        let mut open_cards = vec![];
        let mut resolved_any = false;
        for shown in remarks {
            let (row, text) = self.card(shown);
            if shown.remark.resolved {
                self.resolved.append(&row);
                resolved_any = true;
            } else {
                self.open.append(&row);
                open_cards.push((shown.remark.id, text));
            }
        }
        self.empty.set_visible(open_cards.is_empty());
        self.resolved_title.set_visible(resolved_any);
        self.resolved.set_visible(resolved_any);
        *self.open_cards.borrow_mut() = open_cards;
    }

    /// The card of a remark: its text (edited in place), buttons.
    fn card(&self, shown: &Shown) -> (gtk::ListBoxRow, gtk::TextView) {
        let r = &shown.remark;
        let id = r.id;
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["remark-card"])
            .build();
        if shown.detached {
            column.append(&dim("Its text was deleted."));
        }
        let editable = !self.read_only.get() && !r.resolved;
        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .editable(editable)
            .cursor_visible(editable)
            .accepts_tab(false)
            .css_classes(["remark-text"])
            .build();
        text.buffer().set_text(&r.text);
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
        (row, text)
    }

    /// A card's text: every change goes to the remark; entering it shows
    /// the remark's text in the paper; leaving it empty deletes the remark.
    fn connect_text(&self, text: &gtk::TextView, id: u64) {
        let (shown, on_action) = (self.shown.clone(), self.on_action.clone());
        text.buffer().connect_changed(move |buffer| {
            let new = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
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
            let empty = view.upgrade().is_some_and(|v| {
                let b = v.buffer();
                b.text(&b.start_iter(), &b.end_iter(), false)
                    .trim()
                    .is_empty()
            });
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

/// Cards on old-paper yellow with dark text (in a dark theme too, like the
/// paper); the cards the caret is in outlined.
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
                        padding: 8px; border: 1px solid rgba(120, 100, 40, 0.25); }
         .remark-list > row:selected .remark-card { border-color: rgba(190, 140, 0, 0.9);
                        box-shadow: 0 0 0 1px rgba(190, 140, 0, 0.9); }
         .remark-text, .remark-text text { background: none; color: #2e2a22; }
         .remark-card button { color: #2e2a22; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
