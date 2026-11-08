//! Device-pixel helpers shared by every widget that draws Typst rasters.
//!
//! A raster stays sharp only when it is drawn one texture pixel per device
//! pixel, on the device pixel grid. With fractional scaling (125 %, 175 %)
//! the device scale is the surface's fractional `scale`, not the integer
//! `scale_factor` (which rounds up), and it can change without
//! `scale_factor` changing (moving the window from a 125 % to a 175 %
//! monitor), so widgets watch the surface itself.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, graphene};

/// Device pixels per logical pixel of the surface `widget` is on. Falls
/// back to the integer scale factor before the widget is realized.
pub fn device_scale(widget: &impl IsA<gtk::Widget>) -> f64 {
    widget
        .native()
        .and_then(|n| n.surface())
        .map_or(widget.scale_factor() as f64, |s| s.scale())
}

/// Returns a function that rounds widget coordinates to the nearest device
/// pixel, taking the widget's position in the window surface into account.
pub fn pixel_snapper<W: IsA<gtk::Widget>>(widget: &W) -> impl Fn(f64, f64) -> (f64, f64) + use<W> {
    let scale = device_scale(widget);
    let origin = widget.native().and_then(|native| {
        let (sx, sy) = native.surface_transform();
        widget
            .compute_point(&native, &graphene::Point::new(0.0, 0.0))
            .map(|p| (p.x() as f64 + sx, p.y() as f64 + sy))
    });
    let (ox, oy) = origin.unwrap_or((0.0, 0.0));
    move |x, y| {
        let snap = |v: f64, o: f64| ((v + o) * scale).round() / scale - o;
        (snap(x, ox), snap(y, oy))
    }
}

/// Calls `f` when `widget` is realized and whenever the scale of its
/// surface changes afterwards.
pub fn on_device_scale_change<W: IsA<gtk::Widget>>(widget: &W, f: impl Fn(&W) + 'static) {
    type Conn = Option<(glib::WeakRef<gdk::Surface>, glib::SignalHandlerId)>;
    let f = Rc::new(f);
    let conn: Rc<RefCell<Conn>> = Rc::default();

    let c = conn.clone();
    widget.connect_realize(move |w| {
        if let Some(surface) = w.native().and_then(|n| n.surface()) {
            let weak = w.downgrade();
            let f = f.clone();
            let id = surface.connect_scale_notify(move |_| {
                if let Some(w) = weak.upgrade() {
                    f(&w);
                }
            });
            if let Some((old, id)) = c.replace(Some((surface.downgrade(), id)))
                && let Some(old) = old.upgrade()
            {
                old.disconnect(id);
            }
        }
        f(w);
    });
    widget.connect_unrealize(move |_| {
        if let Some((surface, id)) = conn.take()
            && let Some(surface) = surface.upgrade()
        {
            surface.disconnect(id);
        }
    });
}
