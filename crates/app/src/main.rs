//! Application shell: windows, actions and shortcuts, files, export.

mod file;
mod sample;
mod window;

use adw::prelude::*;
use gtk::gio;

use window::Window;

/// Set by the Flatpak manifest (`.Devel` builds); falls back to the release ID.
const APP_ID: &str = match option_env!("APP_ID") {
    Some(id) => id,
    None => "io.github.molnarandris.SemanticEditor",
};

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(setup_actions);
    app.connect_activate(|app| {
        let window = Window::new(app);
        window.present();
        if let Ok(path) = std::env::var("SEM_SCREENSHOT") {
            screenshot_and_quit(app, &window.window, path);
        }
    });
    app.connect_open(|app, files, _| {
        for file in files {
            let window = Window::new(app);
            window.open_file(file.clone());
            window.present();
        }
    });
    app.run()
}

fn setup_actions(app: &adw::Application) {
    let new = gio::SimpleAction::new("new", None);
    let weak = app.downgrade();
    new.connect_activate(move |_, _| {
        if let Some(app) = weak.upgrade() {
            Window::new(&app).present();
        }
    });
    app.add_action(&new);

    let quit = gio::SimpleAction::new("quit", None);
    let weak = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = weak.upgrade() {
            // Each window asks about unsaved changes on its own.
            for window in app.windows() {
                window.close();
            }
        }
    });
    app.add_action(&quit);

    for (action, accels) in [
        ("app.new", &["<Control>n"][..]),
        ("app.quit", &["<Control>q"]),
        ("win.open", &["<Control>o"]),
        ("win.save", &["<Control>s"]),
        ("win.save-as", &["<Control><Shift>s"]),
        ("win.export-pdf", &["<Control>e"]),
        ("win.export-typ", &["<Control><Shift>e"]),
        ("win.insert-diagram", &["<Control><Shift>d"]),
        ("window.close", &["<Control>w"]),
    ] {
        app.set_accels_for_action(action, accels);
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
