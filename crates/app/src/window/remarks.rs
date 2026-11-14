//! The Remarks pane on the right: a card per remark in the order of the
//! text (author, date, text, replies), the ones the caret is in selected.
//! Clicking a card selects its text; Reply and Resolve on each card;
//! resolved remarks below, dimmed, with Reopen and Delete.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use model::Remark;

/// What a card asks the window to do.
#[derive(Clone, Copy, Debug)]
pub enum RemarkAction {
    Select(u64),
    Reply(u64),
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
    /// The remarks the cards show, to rebuild only on a change.
    shown: RefCell<Vec<Shown>>,
    /// The open cards' remark ids, in list order.
    open_ids: RefCell<Vec<u64>>,
    read_only: Cell<bool>,
    on_action: Callback,
}

impl RemarksPane {
    pub fn new() -> Self {
        let heading = gtk::Label::builder()
            .label("Remarks")
            .xalign(0.0)
            .css_classes(["heading"])
            .margin_start(12)
            .margin_top(12)
            .margin_bottom(6)
            .build();
        let list = || {
            gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["navigation-sidebar"])
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
        widget.append(&heading);
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
            shown: RefCell::default(),
            open_ids: RefCell::default(),
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

    /// An old version: cards without buttons.
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
        let ids = self.open_ids.borrow();
        let mut first = None;
        for (i, id) in ids.iter().enumerate() {
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
        let mut open_ids = vec![];
        let mut resolved_any = false;
        for shown in remarks {
            let row = self.card(shown);
            if shown.remark.resolved {
                self.resolved.append(&row);
                resolved_any = true;
            } else {
                self.open.append(&row);
                open_ids.push(shown.remark.id);
            }
        }
        self.empty.set_visible(open_ids.is_empty());
        self.resolved_title.set_visible(resolved_any);
        self.resolved.set_visible(resolved_any);
        *self.open_ids.borrow_mut() = open_ids;
    }

    /// The card of a remark: author and date, text, replies, buttons.
    fn card(&self, shown: &Shown) -> gtk::ListBoxRow {
        let r = &shown.remark;
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        column.append(&byline(&r.author, &r.date));
        if shown.detached {
            column.append(&dim("Its text was deleted."));
        }
        column.append(&body(&r.text));
        for reply in &r.replies {
            let answer = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .margin_start(12)
                .margin_top(4)
                .build();
            answer.append(&byline(&reply.author, &reply.date));
            answer.append(&body(&reply.text));
            column.append(&answer);
        }
        if !self.read_only.get() {
            let buttons = gtk::Box::builder()
                .spacing(6)
                .halign(gtk::Align::End)
                .build();
            let id = r.id;
            let actions: &[(&str, RemarkAction)] = if r.resolved {
                &[
                    ("Delete", RemarkAction::Delete(id)),
                    ("Reopen", RemarkAction::Resolve(id, false)),
                ]
            } else {
                &[
                    ("Reply", RemarkAction::Reply(id)),
                    ("Resolve", RemarkAction::Resolve(id, true)),
                ]
            };
            for &(label, action) in actions {
                let button = gtk::Button::builder()
                    .label(label)
                    .css_classes(["flat", "caption"])
                    .build();
                let on_action = self.on_action.clone();
                button.connect_clicked(move |_| {
                    if let Some(f) = on_action.borrow().as_ref() {
                        f(action);
                    }
                });
                buttons.append(&button);
            }
            column.append(&buttons);
        }
        let row = gtk::ListBoxRow::builder()
            .child(&column)
            .name(r.id.to_string())
            .build();
        if r.resolved {
            row.add_css_class("dim-label");
        }
        row
    }
}

/// The remark id a card stands for.
fn row_id(row: &gtk::ListBoxRow) -> u64 {
    row.widget_name().parse().unwrap_or_default()
}

fn byline(author: &str, date: &str) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let author = gtk::Label::builder()
        .label(author)
        .xalign(0.0)
        .css_classes(["heading"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    line.append(&author);
    line.append(&dim(date));
    line
}

fn body(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build()
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build()
}
