//! The title block's dialog: title, authors and date as fields. Every change
//! goes to the model at once; consecutive changes are one undo step.

use std::rc::Rc;

use adw::prelude::*;
use model::{NodeId, TitleFields};

use super::Window;

impl Window {
    pub(super) fn open_title_editor(self: &Rc<Self>, block: NodeId) {
        let Some(source) = self.editor.block_source(block) else {
            return;
        };
        let fields = TitleFields::parse(&source);
        let title = adw::EntryRow::builder()
            .title("Title")
            .text(&fields.title)
            .build();
        let authors = adw::EntryRow::builder()
            .title("Authors, separated by commas")
            .text(fields.authors.join(", "))
            .build();
        let date = adw::EntryRow::builder()
            .title("Date, YYYY-MM-DD (optional)")
            .text(&fields.date)
            .build();
        let group = adw::PreferencesGroup::builder()
            .description(
                "Shown at the top of the document and stored as the PDF's metadata. \
                 The title is Typst markup; restyle it in the header with \
                 `show title: …`.",
            )
            .build();
        group.add(&title);
        group.add(&authors);
        group.add(&date);
        let page = adw::PreferencesPage::new();
        page.add(&group);

        let done = gtk::Button::builder()
            .label("Done")
            .css_classes(["suggested-action"])
            .build();
        let header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .build();
        header.pack_end(&done);
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&page));
        let dialog = adw::Dialog::builder()
            .title("Document Title")
            .content_width(480)
            .child(&content)
            .build();

        let rows = Rc::new([title, authors, date]);
        for row in rows.iter() {
            let rows = rows.clone();
            let editor = self.editor.clone();
            row.connect_changed(move |_| {
                let [title, authors, date] = &*rows;
                let fields = TitleFields {
                    title: title.text().into(),
                    authors: authors
                        .text()
                        .split(',')
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .map(String::from)
                        .collect(),
                    date: date.text().trim().into(),
                };
                if fields.date.is_empty() || fields.ymd().is_some() {
                    date.remove_css_class("error");
                } else {
                    date.add_css_class("error");
                }
                editor.set_block_source(block, &fields.source());
            });
            let dialog = dialog.downgrade();
            row.connect_entry_activated(move |_| {
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            });
        }
        let weak = dialog.downgrade();
        done.connect_clicked(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.close();
            }
        });
        let editor = self.editor.clone();
        dialog.connect_closed(move |_| {
            editor.grab_focus();
        });
        dialog.present(Some(&self.window));
        rows[0].grab_focus();
    }
}
