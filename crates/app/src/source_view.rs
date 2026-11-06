//! The source pane's text view: a `sourceview5::View` that also shows where
//! the editor's caret is while the pane does not have focus (GTK draws the
//! text cursor only in a focused view).

use gtk::glib;
use gtk::subclass::prelude::*;

mod imp {
    use std::cell::Cell;

    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{glib, graphene};
    use sourceview5::subclass::prelude::*;

    #[derive(Default)]
    pub struct CaretView {
        /// Char offset of the mirrored caret.
        pub caret: Cell<Option<i32>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CaretView {
        const NAME: &'static str = "SemCaretView";
        type Type = super::CaretView;
        type ParentType = sourceview5::View;
    }

    impl ObjectImpl for CaretView {}

    impl WidgetImpl for CaretView {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.parent_snapshot(snapshot);
            let view = self.obj();
            if view.has_focus() {
                return; // GTK draws the real cursor.
            }
            let Some(offset) = self.caret.get() else {
                return;
            };
            let iter = view.buffer().iter_at_offset(offset);
            let r = view.iter_location(&iter);
            let (x, y) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x(), r.y());
            let bar = graphene::Rect::new(x as f32 - 1.0, y as f32, 2.0, r.height() as f32);
            snapshot.append_color(&view.color(), &bar);
        }
    }

    impl TextViewImpl for CaretView {}
    impl ViewImpl for CaretView {}
}

glib::wrapper! {
    pub struct CaretView(ObjectSubclass<imp::CaretView>)
        @extends sourceview5::View, gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::AccessibleText, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Scrollable;
}

impl CaretView {
    pub fn new(buffer: &sourceview5::Buffer) -> Self {
        glib::Object::builder().property("buffer", buffer).build()
    }

    /// Show the editor's caret at char `offset` (drawn while unfocused).
    pub fn set_caret(&self, offset: Option<i32>) {
        self.imp().caret.set(offset);
        gtk::prelude::WidgetExt::queue_draw(self);
    }
}
