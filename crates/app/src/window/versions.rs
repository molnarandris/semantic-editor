//! The Versions page of the sidebar: the document's named versions, then
//! every save grouped by day (collapsed, the latest day open), each with
//! its time and tags. Clicking a version opens it (read-only).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use git2::Oid;
use gtk::glib;

use crate::history::Version;

type Callback = Rc<RefCell<Option<Box<dyn Fn(Oid)>>>>;
type FileCallback = Rc<RefCell<Option<Box<dyn Fn(&str)>>>>;

pub struct VersionsPane {
    pub widget: gtk::ScrolledWindow,
    named: gtk::ListBox,
    saves: gtk::ListBox,
    named_title: gtk::Label,
    empty: gtk::Label,
    /// Called with a version picked in the list.
    on_pick: Callback,
    scratch_title: gtk::Label,
    scratch_hint: gtk::Label,
    scratch: gtk::ListBox,
    /// The scratch files shown (path, used).
    shown_scratch: RefCell<Vec<(String, bool)>>,
    on_keep: FileCallback,
    on_delete: FileCallback,
}

impl VersionsPane {
    /// The pane; its button runs the window action `win.name-version`.
    pub fn new() -> Self {
        let name_current = gtk::Button::builder()
            .label("Name Current Version…")
            .action_name("win.name-version")
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .build();
        let title = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .css_classes(["heading"])
                .margin_start(12)
                .margin_top(12)
                .margin_bottom(4)
                .build()
        };
        let list = || {
            gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["navigation-sidebar"])
                .build()
        };
        let named_title = title("Named Versions");
        let (named, saves) = (list(), list());
        let empty = gtk::Label::builder()
            .label("No saved versions yet.\nEvery save becomes one.")
            .css_classes(["dim-label"])
            .justify(gtk::Justification::Center)
            .margin_top(24)
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&name_current);
        column.append(&named_title);
        column.append(&named);
        column.append(&title("Saves"));
        column.append(&saves);
        column.append(&empty);
        let scratch_title = title("Scratch Files");
        let scratch_hint = gtk::Label::builder()
            .label("Kept with the document, not in its versions. Keep one to version it.")
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .margin_start(12)
            .margin_end(12)
            .build();
        let scratch = list();
        column.append(&scratch_title);
        column.append(&scratch_hint);
        column.append(&scratch);
        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&column)
            .build();
        let on_pick: Callback = Rc::default();
        for l in [&named, &saves] {
            let on_pick = on_pick.clone();
            l.connect_row_activated(move |_, row| {
                if let (Some(id), Some(f)) = (row_id(row), on_pick.borrow().as_ref()) {
                    f(id);
                }
            });
        }
        let this = Self {
            widget,
            named,
            saves,
            named_title,
            empty,
            on_pick,
            scratch_title,
            scratch_hint,
            scratch,
            shown_scratch: RefCell::default(),
            on_keep: Rc::default(),
            on_delete: Rc::default(),
        };
        this.update_scratch(&[]);
        this
    }

    /// Call `f` with a scratch file to keep (version).
    pub fn connect_keep(&self, f: impl Fn(&str) + 'static) {
        *self.on_keep.borrow_mut() = Some(Box::new(f));
    }

    /// Call `f` with an unused scratch file to delete.
    pub fn connect_delete(&self, f: impl Fn(&str) + 'static) {
        *self.on_delete.borrow_mut() = Some(Box::new(f));
    }

    /// Show the scratch files (path, whether the document uses it); the
    /// section hides when there are none.
    pub fn update_scratch(&self, files: &[(String, bool)]) {
        if *self.shown_scratch.borrow() == files && !files.is_empty() {
            return;
        }
        *self.shown_scratch.borrow_mut() = files.to_vec();
        self.scratch.remove_all();
        for w in [
            self.scratch_title.upcast_ref::<gtk::Widget>(),
            self.scratch_hint.upcast_ref(),
            self.scratch.upcast_ref(),
        ] {
            w.set_visible(!files.is_empty());
        }
        for (path, used) in files {
            let name = path.split_once('/').map_or(path.as_str(), |(_, n)| n);
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(name))
                .subtitle(if *used {
                    "In the document"
                } else {
                    "Not in the document"
                })
                .build();
            let keep = gtk::Button::builder()
                .label("Keep")
                .tooltip_text("Version it: move it to the document’s pictures")
                .valign(gtk::Align::Center)
                .build();
            let delete = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Delete (only when not in the document)")
                .valign(gtk::Align::Center)
                .sensitive(!*used)
                .css_classes(["flat"])
                .build();
            for (button, callback) in [(&keep, &self.on_keep), (&delete, &self.on_delete)] {
                let (callback, path) = (callback.clone(), path.clone());
                button.connect_clicked(move |_| {
                    if let Some(f) = callback.borrow().as_ref() {
                        f(&path);
                    }
                });
            }
            row.add_suffix(&keep);
            row.add_suffix(&delete);
            self.scratch.append(&row);
        }
    }

    /// Call `f` with a version picked in the list.
    pub fn connect_pick(&self, f: impl Fn(Oid) + 'static) {
        *self.on_pick.borrow_mut() = Some(Box::new(f));
    }

    /// Show `versions` (newest first).
    pub fn update(&self, versions: &[Version]) {
        for list in [&self.named, &self.saves] {
            list.remove_all();
        }
        let named: Vec<&Version> = versions.iter().filter(|v| !v.tags.is_empty()).collect();
        self.named_title.set_visible(!named.is_empty());
        self.named.set_visible(!named.is_empty());
        self.empty.set_visible(versions.is_empty());
        for v in named {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&v.tags.join(", ")))
                .subtitle(format!("{}, {}", day_of(v), time_of(v)))
                .activatable(true)
                .build();
            set_row_id(row.upcast_ref(), v.id);
            self.named.append(&row);
        }
        // Saves by day, newest first; the latest day open.
        let mut days: Vec<(String, Vec<&Version>)> = vec![];
        for v in versions {
            let day = day_of(v);
            match days.last_mut() {
                Some((d, list)) if *d == day => list.push(v),
                _ => days.push((day, vec![v])),
            }
        }
        for (i, (day, list)) in days.iter().enumerate() {
            let count = match list.len() {
                1 => "1 save".to_string(),
                n => format!("{n} saves"),
            };
            let expander = adw::ExpanderRow::builder()
                .title(day.as_str())
                .subtitle(count)
                .expanded(i == 0)
                .build();
            for v in list {
                let tags = match v.tags.is_empty() {
                    true => String::new(),
                    false => format!(" · {}", v.tags.join(", ")),
                };
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&format!("{}{tags}", time_of(v))))
                    .activatable(true)
                    .build();
                set_row_id(row.upcast_ref(), v.id);
                let on_pick = self.on_pick.clone();
                let id = v.id;
                row.connect_activated(move |_| {
                    if let Some(f) = on_pick.borrow().as_ref() {
                        f(id);
                    }
                });
                expander.add_row(&row);
            }
            self.saves.append(&expander);
        }
    }
}

/// The version a row stands for (its widget name).
fn row_id(row: &gtk::ListBoxRow) -> Option<Oid> {
    Oid::from_str(&row.widget_name()).ok()
}

fn set_row_id(row: &gtk::ListBoxRow, id: Oid) {
    row.set_widget_name(&id.to_string());
}

fn local(v: &Version) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(v.time).ok()
}

/// When a version was saved: "Today, 14:03", "3 November 2026, 09:12".
pub fn label(v: &Version) -> String {
    format!("{}, {}", day_of(v), time_of(v))
}

/// "Today", "Yesterday" or the date, in local time.
fn day_of(v: &Version) -> String {
    let Some(t) = local(v) else {
        return String::new();
    };
    let now = glib::DateTime::now_local().ok();
    let same = |a: &glib::DateTime, b: &glib::DateTime| {
        (a.year(), a.day_of_year()) == (b.year(), b.day_of_year())
    };
    if let Some(now) = now {
        if same(&t, &now) {
            return "Today".into();
        }
        if now.add_days(-1).is_ok_and(|y| same(&t, &y)) {
            return "Yesterday".into();
        }
    }
    t.format("%e %B %Y")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The time of day, "14:03".
fn time_of(v: &Version) -> String {
    local(v)
        .and_then(|t| t.format("%H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}
