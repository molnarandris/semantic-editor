//! The selection pop-up: icon buttons for the app's actions (Add Remark,
//! Ask Claude) above a selection, once it has stayed the same for a moment.
//! It never takes the focus, so copying, pasting and typing go on in the
//! editor; any change of the selection, a scroll or leaving the editor
//! hides it.

use std::time::Duration;

use super::*;

/// How long a selection has to stay before the pop-up shows.
const PAUSE: Duration = Duration::from_millis(500);

impl SemEditor {
    /// The pop-up's buttons: (icon, tooltip, action name) each; none, no
    /// pop-up.
    pub(crate) fn set_selection_actions(&self, actions: &[(&str, &str, &str)]) {
        self.hide_selection_popup();
        if let Some(old) = self.sel_popup.take() {
            old.unparent();
        }
        if actions.is_empty() {
            return;
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        for &(icon, tooltip, action) in actions {
            let button = gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .action_name(action)
                .can_focus(false)
                .focus_on_click(false)
                .css_classes(["flat"])
                .build();
            let weak = self.obj().downgrade();
            button.connect_clicked(move |_| {
                if let Some(obj) = weak.upgrade() {
                    obj.imp().hide_selection_popup();
                }
            });
            row.append(&button);
        }
        let popup = gtk::Popover::builder()
            .child(&row)
            .autohide(false)
            .can_focus(false)
            .position(gtk::PositionType::Top)
            .build();
        popup.set_parent(&*self.obj());
        *self.sel_popup.borrow_mut() = Some(popup);
    }

    /// After a command, or a scroll (`moved`): if the selection changed or
    /// moved, hide the pop-up and show it again once it rests.
    pub(super) fn update_selection_popup(&self, moved: bool) {
        if self.sel_popup.borrow().is_none() {
            return;
        }
        let key = {
            let ed = self.editor.borrow();
            ed.selection()
                .map(|_| (ed.anchor().clone(), ed.caret().clone()))
        };
        if !moved && *self.sel_key.borrow() == key {
            return;
        }
        self.hide_selection_popup();
        let has_selection = key.is_some();
        *self.sel_key.borrow_mut() = key;
        if !has_selection {
            return;
        }
        let weak = self.obj().downgrade();
        let id = glib::timeout_add_local_once(PAUSE, move || {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                imp.sel_timer.take();
                imp.show_selection_popup();
            }
        });
        *self.sel_timer.borrow_mut() = Some(id);
    }

    pub(super) fn hide_selection_popup(&self) {
        if let Some(id) = self.sel_timer.take() {
            id.remove();
        }
        if let Some(popup) = self.sel_popup.borrow().as_ref() {
            popup.popdown();
        }
    }

    /// Show the pop-up above the selection's first visible line, if the
    /// editor has the focus.
    fn show_selection_popup(&self) {
        if !self.obj().has_focus() {
            return;
        }
        let Some(rect) = self.selection_top() else {
            return;
        };
        if let Some(popup) = self.sel_popup.borrow().as_ref() {
            popup.set_pointing_to(Some(&gdk::Rectangle::new(
                rect.x() as i32,
                rect.y() as i32,
                rect.width().ceil() as i32,
                rect.height().ceil() as i32,
            )));
            popup.popup();
        }
    }

    /// The selection's first line in view, in widget coordinates.
    fn selection_top(&self) -> Option<graphene::Rect> {
        let scroll = self.scroll();
        let x0 = self.page_x(self.obj().width() as f64);
        let selection = self.editor.borrow().selection()?;
        let rects = match selection {
            model::SelRange::Blocks { start, end } => {
                let zoom = self.zoom.get();
                let tops = self.block_tops();
                let views = self.views.borrow();
                (start..end)
                    .filter_map(|i| {
                        let (top, view) = (tops.get(i)?, views.get(i)?);
                        Some(graphene::Rect::new(
                            x0 as f32,
                            (top - scroll) as f32,
                            (self.page_width_pt.get() * zoom) as f32,
                            (view.height_pt() * zoom) as f32,
                        ))
                    })
                    .collect()
            }
            _ => {
                let (first, last) = self.editor.borrow().selection_bounds()?;
                self.range_rects(&first, &last, x0, scroll)
            }
        };
        let height = self.obj().height() as f32;
        rects
            .into_iter()
            .filter(|r| r.y() + r.height() > 0.0 && r.y() < height)
            .min_by(|a, b| a.y().total_cmp(&b.y()))
            .map(|r| {
                // A band cut at the view's top points at the top edge.
                let y = r.y().max(0.0);
                graphene::Rect::new(r.x(), y, r.width(), r.y() + r.height() - y)
            })
    }
}
