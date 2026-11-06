//! Application shell: window, header bar, status bar, files, export.

mod sample;

use adw::prelude::*;
use editor_widget::SemEditor;

/// Set by the Flatpak manifest (`.Devel` builds); falls back to the release ID.
const APP_ID: &str = match option_env!("APP_ID") {
    Some(id) => id,
    None => "io.github.molnarandris.SemanticEditor",
};

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &adw::Application) {
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("notes.sem", "Semantic Editor"))
        .build();

    let editor = SemEditor::new();
    editor.set_document(sample::document());
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&editor)
        .build();

    let status = gtk::Label::builder()
        .label("paragraph")
        .xalign(0.0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .css_classes(["caption", "dim-label"])
        .build();

    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(&scroller));
    content.add_bottom_bar(&status);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("notes.sem — Semantic Editor")
        .default_width(900)
        .default_height(760)
        .content(&content)
        .build();
    if APP_ID.ends_with(".Devel") {
        window.add_css_class("devel");
    }
    window.present();
    editor.grab_focus();

    if let Ok(path) = std::env::var("SEM_SCREENSHOT") {
        screenshot_and_quit(app, &window, path);
    }
}

/// Debug aid: save a PNG of the window once the first renders are in, then
/// quit. Used to check rendering without a human looking at the screen.
fn screenshot_and_quit(app: &adw::Application, window: &adw::ApplicationWindow, path: String) {
    let (app, window) = (app.clone(), window.clone());
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        let (w, h) = (window.width() as f64, window.height() as f64);
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, w, h);
        let renderer = window.native().and_then(|n| n.renderer());
        match (snapshot.to_node(), renderer) {
            (Some(node), Some(renderer)) => {
                let texture = renderer.render_texture(&node, None);
                if let Err(e) = texture.save_to_png(&path) {
                    eprintln!("screenshot failed: {e}");
                }
            }
            _ => eprintln!("screenshot failed: nothing rendered"),
        }
        app.quit();
    });
}
