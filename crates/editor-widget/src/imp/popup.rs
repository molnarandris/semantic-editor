//! The completion list in a pop-up below the caret: each label (or name)
//! with what it is, the chosen one selected. A real pop-up, so that it is
//! not cut to the widget (a remark's card is only as tall as its text).
//! It never takes the focus: the keys stay with the editor.

use super::*;

/// Rows shown at a time; the chosen one is kept among them.
const SHOWN_ROWS: usize = 8;

impl SemEditor {
    /// Make the pop-up (once, at construction).
    pub(super) fn setup_popup(&self) {
        let obj = self.obj();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .build();
        let weak = obj.downgrade();
        list.connect_row_activated(move |_, row| {
            if let Some(obj) = weak.upgrade() {
                obj.imp().choose_completion(&row.widget_name());
            }
        });
        let popup = gtk::Popover::builder()
            .child(&list)
            .autohide(false)
            .has_arrow(false)
            .can_focus(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::Start)
            .build();
        popup.set_parent(&*obj);
        let _ = self.popup.set((popup, list));
    }

    /// Show the completions for what is typed at the caret, or hide the
    /// pop-up if there are none (or the editor is not focused).
    pub(super) fn update_completions(&self) {
        let Some((popup, list)) = self.popup.get() else {
            return;
        };
        let completions = self.completions();
        let caret = self
            .caret_rect()
            .filter(|_| self.obj().has_focus() && !completions.is_empty());
        let Some(caret) = caret else {
            popup.popdown();
            return;
        };
        list.remove_all();
        let chosen = self.completion.get().min(completions.len() - 1);
        let shown = completions.len().min(SHOWN_ROWS);
        let first = chosen.saturating_sub(shown - 1);
        let labels = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        for (label, desc) in completions.iter().skip(first).take(shown) {
            let name = gtk::Label::builder().label(label).xalign(0.0).build();
            labels.add_widget(&name);
            let what = gtk::Label::builder()
                .label(desc)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build();
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 18);
            row_box.append(&name);
            row_box.append(&what);
            let row = gtk::ListBoxRow::builder()
                .child(&row_box)
                .name(label)
                .can_focus(false)
                .build();
            list.append(&row);
        }
        list.select_row(list.row_at_index((chosen - first) as i32).as_ref());
        popup.set_pointing_to(Some(&gdk::Rectangle::new(
            caret.x() as i32,
            caret.y() as i32,
            caret.width().ceil() as i32,
            caret.height().ceil() as i32,
        )));
        popup.popup();
    }
}
