use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gtk::glib::subclass::Signal;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use model::{CursorPath, Document, Editor, NodeId};
use typst_bridge::{
    BLOCK_PAD_PT, BlockLayout, CompileJob, CompileResult, Raster, Worker, serialize_block,
};

use crate::caret::{self, Stop};

/// Logical pixels per typographic point: default, limits, and the factor
/// of one zoom step.
pub const DEFAULT_ZOOM: f64 = 1.75;
const MIN_ZOOM: f64 = 0.75;
const MAX_ZOOM: f64 = 5.0;
const ZOOM_STEP: f64 = 1.1;
/// Page width limits in points (the page shrinks with the window).
const MAX_PAGE_WIDTH_PT: f64 = 460.0;
const MIN_PAGE_WIDTH_PT: f64 = 120.0;
/// Horizontal space around the page and vertical space above/below, in px.
const SIDE_MARGIN_PX: f64 = 32.0;
const TOP_MARGIN_PX: f64 = 40.0;
/// Paper padding around the text column, in pt.
const PAPER_PAD_PT: f64 = 24.0;
/// Vertical space between blocks, in pt.
const BLOCK_SPACING_PT: f64 = 8.0;
/// Height of a block that is empty or not rendered yet, in pt.
const MIN_BLOCK_HEIGHT_PT: f64 = 13.0;
/// Caret width in px and blink half-period.
const CARET_WIDTH_PX: f64 = 1.5;
const BLINK_INTERVAL: Duration = Duration::from_millis(600);

/// Render state of one block.
struct BlockView {
    id: NodeId,
    /// Snippet of the last submitted job, to skip redundant compiles.
    snippet: String,
    /// Generation of the last submitted job.
    submitted: u64,
    /// Generation of the result currently shown.
    shown: u64,
    texture: Option<gdk::Texture>,
    /// Device pixels per point the texture was rendered at.
    texture_ppp: f64,
    layout: Option<BlockLayout>,
    /// Message of the last failed compile; the last good render stays.
    error: Option<String>,
}

impl BlockView {
    fn new(id: NodeId) -> Self {
        Self {
            id,
            snippet: String::new(),
            submitted: 0,
            shown: 0,
            texture: None,
            texture_ppp: 1.0,
            layout: None,
            error: None,
        }
    }

    fn height_pt(&self) -> f64 {
        self.layout
            .as_ref()
            .map_or(0.0, |l| l.height)
            .max(MIN_BLOCK_HEIGHT_PT)
    }
}

#[derive(Default)]
pub struct SemEditor {
    pub(crate) editor: RefCell<Editor>,
    views: RefCell<Vec<BlockView>>,
    worker: OnceCell<Worker>,
    generation: Cell<u64>,
    /// Page width and pixels per point the current renders were requested at.
    page_width_pt: Cell<f64>,
    pixel_per_pt: Cell<f64>,
    /// Logical pixels per point.
    zoom: Cell<f64>,
    /// Zoom when a pinch gesture began.
    pinch_start: Cell<f64>,

    im: OnceCell<gtk::IMMulticontext>,
    caret_on: Cell<bool>,
    blink: RefCell<Option<glib::SourceId>>,
    /// Column kept while moving up and down, in pt.
    goal_x: Cell<Option<f64>>,
    /// Scroll the caret into view when its block is rendered.
    reveal: Cell<bool>,

    hadjustment: RefCell<Option<gtk::Adjustment>>,
    vadjustment: RefCell<Option<gtk::Adjustment>>,
    hscroll_policy: Cell<Option<gtk::ScrollablePolicy>>,
    vscroll_policy: Cell<Option<gtk::ScrollablePolicy>>,
}

#[glib::object_subclass]
impl ObjectSubclass for SemEditor {
    const NAME: &'static str = "SemEditor";
    type Type = super::SemEditor;
    type ParentType = gtk::Widget;
    type Interfaces = (gtk::Scrollable,);

    fn class_init(klass: &mut Self::Class) {
        klass.set_css_name("semeditor");
    }
}

impl ObjectImpl for SemEditor {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPS: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
        PROPS.get_or_init(|| {
            [
                "hadjustment",
                "vadjustment",
                "hscroll-policy",
                "vscroll-policy",
            ]
            .into_iter()
            .map(glib::ParamSpecOverride::for_interface::<gtk::Scrollable>)
            .collect()
        })
    }

    fn signals() -> &'static [Signal] {
        static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
        // Emitted after every command: caret, selection or document changed.
        SIGNALS.get_or_init(|| vec![Signal::builder("caret-moved").build()])
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "hadjustment" => self.set_adjustment(&self.hadjustment, value.get().unwrap()),
            "vadjustment" => self.set_adjustment(&self.vadjustment, value.get().unwrap()),
            "hscroll-policy" => self.hscroll_policy.set(Some(value.get().unwrap())),
            "vscroll-policy" => self.vscroll_policy.set(Some(value.get().unwrap())),
            name => unreachable!("unknown property {name}"),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        let policy = |p: &Cell<Option<gtk::ScrollablePolicy>>| {
            p.get().unwrap_or(gtk::ScrollablePolicy::Minimum).to_value()
        };
        match pspec.name() {
            "hadjustment" => self.hadjustment.borrow().to_value(),
            "vadjustment" => self.vadjustment.borrow().to_value(),
            "hscroll-policy" => policy(&self.hscroll_policy),
            "vscroll-policy" => policy(&self.vscroll_policy),
            name => unreachable!("unknown property {name}"),
        }
    }

    fn constructed(&self) {
        self.parent_constructed();
        let obj = self.obj();
        obj.set_focusable(true);
        obj.set_focus_on_click(true);
        obj.set_overflow(gtk::Overflow::Hidden);
        obj.set_cursor_from_name(Some("text"));
        self.zoom.set(DEFAULT_ZOOM);

        // Results arrive on the compile thread; hop to the main loop.
        let weak = glib::SendWeakRef::from(obj.downgrade());
        let worker = Worker::spawn(move |result| {
            let weak = weak.clone();
            glib::MainContext::default().invoke(move || {
                if let Some(editor) = weak.upgrade() {
                    editor.imp().on_result(result);
                }
            });
        });
        let _ = self.worker.set(worker);

        self.editor
            .borrow_mut()
            .set_symbol_resolver(typst_bridge::resolve_symbol);
        obj.connect_scale_factor_notify(|obj| obj.queue_allocate());
        self.setup_input();
    }

    fn dispose(&self) {
        if let Some(im) = self.im.get() {
            im.set_client_widget(None::<&gtk::Widget>);
        }
        self.stop_blink();
    }
}

impl WidgetImpl for SemEditor {
    fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
        let zoom = self.zoom.get();
        match orientation {
            gtk::Orientation::Horizontal => {
                let min = MIN_PAGE_WIDTH_PT * zoom + 2.0 * SIDE_MARGIN_PX;
                let nat = MAX_PAGE_WIDTH_PT * zoom + 2.0 * (SIDE_MARGIN_PX + PAPER_PAD_PT * zoom);
                (min as i32, nat as i32, -1, -1)
            }
            _ => (0, self.content_height() as i32, -1, -1),
        }
    }

    fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
        let zoom = self.zoom.get();
        let page = ((width as f64 - 2.0 * (SIDE_MARGIN_PX + PAPER_PAD_PT * zoom)) / zoom)
            .clamp(MIN_PAGE_WIDTH_PT, MAX_PAGE_WIDTH_PT)
            .floor();
        let ppp = zoom * self.device_scale();
        if page != self.page_width_pt.get() || ppp != self.pixel_per_pt.get() {
            self.page_width_pt.set(page);
            self.pixel_per_pt.set(ppp);
            self.submit_blocks(true);
        }
        self.configure_adjustments(width as f64, height as f64);
    }

    fn snapshot(&self, snapshot: &gtk::Snapshot) {
        let zoom = self.zoom.get();
        let obj = self.obj();
        let (width, height) = (obj.width() as f64, obj.height() as f64);
        let scroll = self.scroll();
        let page_px = self.page_width_pt.get() * zoom;
        let x0 = self.page_x(width);
        let pad = PAPER_PAD_PT * zoom;

        // Paper.
        let paper = graphene::Rect::new(
            (x0 - pad) as f32,
            (TOP_MARGIN_PX - pad - scroll) as f32,
            (page_px + 2.0 * pad) as f32,
            (self.content_height() - 2.0 * TOP_MARGIN_PX + 2.0 * pad) as f32,
        );
        snapshot.append_color(&gdk::RGBA::WHITE, &paper);
        if obj.has_focus() {
            self.snapshot_highlights(snapshot, x0, scroll);
        }

        let snap = self.pixel_snapper();
        let mut y = TOP_MARGIN_PX - scroll;
        for view in self.views.borrow().iter() {
            let h = view.height_pt() * zoom;
            if y + h >= 0.0 && y <= height {
                if let Some(texture) = &view.texture {
                    // The raster has a transparent pad around the content.
                    // Draw it one texture pixel per device pixel, on the
                    // pixel grid: any resampling blurs the thin glyphs.
                    let pad = BLOCK_PAD_PT * zoom;
                    let (px, py) = snap(x0 - pad, y - pad);
                    let per_px = zoom / view.texture_ppp;
                    let rect = graphene::Rect::new(
                        px as f32,
                        py as f32,
                        (texture.width() as f64 * per_px) as f32,
                        (texture.height() as f64 * per_px) as f32,
                    );
                    snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Linear, &rect);
                }
                if view.error.is_some() {
                    let marker =
                        graphene::Rect::new((x0 - 14.0) as f32, (y + 5.0) as f32, 6.0, 6.0);
                    snapshot.append_color(&gdk::RGBA::new(0.88, 0.11, 0.14, 1.0), &marker);
                }
            }
            y += h + BLOCK_SPACING_PT * zoom;
        }

        if obj.has_focus()
            && self.caret_on.get()
            && let Some(r) = self.caret_rect()
        {
            snapshot.append_color(&gdk::RGBA::new(0.1, 0.1, 0.12, 1.0), &r);
        }
    }
}

impl ScrollableImpl for SemEditor {}

/// Editing commands that keys map to.
#[derive(Clone, Copy, Debug)]
enum Command {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    Backspace,
    Delete,
    Enter,
    Undo,
    Redo,
    Escape,
    NextSlot,
    PrevSlot,
    /// Ctrl+1…6: heading level; Ctrl+0: paragraph.
    Heading(Option<u8>),
    /// Ctrl+plus / Ctrl+minus.
    Zoom(f64),
}

impl SemEditor {
    pub(crate) fn set_document(&self, doc: Document) {
        let mut editor = Editor::new(doc);
        editor.set_symbol_resolver(typst_bridge::resolve_symbol);
        *self.editor.borrow_mut() = editor;
        self.views.borrow_mut().clear();
        self.sync_views();
        self.submit_blocks(true);
        self.obj().queue_resize();
        self.obj().emit_by_name::<()>("caret-moved", &[]);
    }

    fn setup_input(&self) {
        let obj = self.obj();

        let im = gtk::IMMulticontext::new();
        im.set_client_widget(Some(&*obj));
        let weak = obj.downgrade();
        im.connect_commit(move |_, text| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                imp.editor.borrow_mut().insert_text(text);
                imp.after_command(true);
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_im_context(Some(&im));
        let weak = obj.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(obj) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match command_for(key, state) {
                Some(cmd) => {
                    obj.imp().run(cmd);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        obj.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let weak = obj.downgrade();
        focus.connect_enter(move |_| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                if let Some(im) = imp.im.get() {
                    im.focus_in();
                }
                imp.restart_blink();
            }
        });
        let weak = obj.downgrade();
        focus.connect_leave(move |_| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                if let Some(im) = imp.im.get() {
                    im.focus_out();
                }
                imp.stop_blink();
                obj.queue_draw();
            }
        });
        obj.add_controller(focus);

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let weak = obj.downgrade();
        click.connect_pressed(move |_, _, x, y| {
            if let Some(obj) = weak.upgrade() {
                obj.grab_focus();
                let imp = obj.imp();
                if let Some(cursor) = imp.locate(x, y) {
                    imp.editor.borrow_mut().set_caret(cursor);
                    imp.goal_x.set(None);
                    imp.after_command(false);
                }
            }
        });
        obj.add_controller(click);

        // Ctrl+wheel and touchpad pinch zoom.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = obj.downgrade();
        scroll.connect_scroll(move |ctrl, _, dy| {
            let ctrl_held = ctrl
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            match weak.upgrade() {
                Some(obj) if ctrl_held => {
                    obj.imp().zoom_by(-dy.signum());
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        obj.add_controller(scroll);
        let pinch = gtk::GestureZoom::new();
        let weak = obj.downgrade();
        pinch.connect_begin(move |_, _| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                imp.pinch_start.set(imp.zoom.get());
            }
        });
        let weak = obj.downgrade();
        pinch.connect_scale_changed(move |_, scale| {
            if let Some(obj) = weak.upgrade() {
                let imp = obj.imp();
                imp.set_zoom(imp.pinch_start.get() * scale);
            }
        });
        obj.add_controller(pinch);

        let _ = self.im.set(im);
    }

    fn run(&self, cmd: Command) {
        let vertical = matches!(cmd, Command::Up | Command::Down);
        if !vertical {
            self.goal_x.set(None);
        }
        // Whether a command changed anything is not needed: the view syncs
        // by comparing sources.
        let _ = {
            let mut ed = self.editor.borrow_mut();
            match cmd {
                Command::Left => {
                    ed.move_left();
                    false
                }
                Command::Right => {
                    ed.move_right();
                    false
                }
                Command::Backspace => ed.backspace(),
                Command::Delete => ed.delete(),
                Command::Enter => ed.enter(),
                Command::Undo => ed.undo(),
                Command::Redo => ed.redo(),
                Command::Escape => ed.escape(),
                Command::NextSlot => ed.next_slot(false),
                Command::Heading(level) => ed.set_heading(level),
                Command::Zoom(steps) => {
                    drop(ed);
                    self.zoom_by(steps);
                    return;
                }
                Command::PrevSlot => ed.next_slot(true),
                Command::DocStart => {
                    let c = ed.doc().block_start(0);
                    ed.set_caret(c);
                    false
                }
                Command::DocEnd => {
                    let c = ed.doc().block_end(ed.doc().blocks.len() - 1);
                    ed.set_caret(c);
                    false
                }
                Command::Up | Command::Down => {
                    let target = ed.vertical_target(matches!(cmd, Command::Down));
                    drop(ed);
                    let next = match target {
                        Some(t) => Some(self.column_in_row(t)),
                        None => self.geometric_move(cmd),
                    };
                    if let Some(c) = next {
                        self.editor.borrow_mut().set_caret(c);
                    }
                    false
                }
                Command::LineStart | Command::LineEnd => {
                    drop(ed);
                    if let Some(c) = self.geometric_move(cmd) {
                        self.editor.borrow_mut().set_caret(c);
                    }
                    false
                }
            }
        };
        self.after_command(true);
    }

    /// Bring the view in line with the model after a command. Any command
    /// may change the document (moving the caret finishes a word, `chi` →
    /// χ); only blocks whose source changed are recompiled.
    fn after_command(&self, reveal: bool) {
        self.sync_views();
        self.submit_blocks(false);
        self.obj().queue_resize();
        if reveal {
            self.reveal.set(true);
            self.reveal_caret();
        }
        self.update_im_location();
        self.restart_blink();
        self.obj().emit_by_name::<()>("caret-moved", &[]);
    }

    /// Select the structure of breadcrumb entry `path_len`.
    pub(crate) fn select_crumb(&self, path_len: usize) {
        let changed = self.editor.borrow_mut().select_crumb(path_len);
        if changed {
            self.after_command(false);
        }
    }

    /// The caret target `t` (offset 0 of some row) moved to the column of
    /// the caret.
    fn column_in_row(&self, t: CursorPath) -> CursorPath {
        let Some((index, current)) = self.caret_stop() else {
            return t;
        };
        let x = self.goal_x.get().unwrap_or(current.x);
        self.goal_x.set(Some(x));
        let stops = self.block_stops(index);
        caret::nearest_in_row(&stops, &t.path, x).map_or(t, |s| s.cursor.clone())
    }

    /// Current structure and selection, drawn under the text.
    fn snapshot_highlights(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let ed = self.editor.borrow();
        let (context, selected) = (ed.context_ids(), ed.selected_ids());
        if context.is_empty() && selected.is_empty() {
            return;
        }
        let Some(index) = ed.doc().block_index(ed.caret().block) else {
            return;
        };
        let Some(&top) = self.block_tops().get(index) else {
            return;
        };
        let views = self.views.borrow();
        let Some(layout) = views.get(index).and_then(|v| v.layout.as_ref()) else {
            return;
        };
        let rect = |id: NodeId, pad: f64| {
            layout.box_of(id).map(|r| {
                graphene::Rect::new(
                    (x0 + r.x * zoom - pad) as f32,
                    (top + r.y * zoom - scroll - pad) as f32,
                    (r.w * zoom + 2.0 * pad) as f32,
                    (r.h * zoom + 2.0 * pad) as f32,
                )
            })
        };
        let blue = |a: f32| gdk::RGBA::new(0.21, 0.52, 0.89, a);
        if let Some(r) = context.first().and_then(|&id| rect(id, 2.0)) {
            snapshot.append_color(&blue(0.07), &r);
        }
        if context.len() > 1
            && let Some(r) = context.last().and_then(|&id| rect(id, 1.5))
        {
            snapshot.append_color(&blue(0.08), &r);
            let outline = gsk::RoundedRect::from_rect(r, 2.0);
            snapshot.append_border(&outline, &[1.0; 4], &[blue(0.55); 4]);
        }
        for r in selected.iter().filter_map(|&id| rect(id, 1.0)) {
            snapshot.append_color(&blue(0.3), &r);
        }
    }

    /// Match the views to the document's blocks, keeping renders by id.
    fn sync_views(&self) {
        let ed = self.editor.borrow();
        let mut views = self.views.borrow_mut();
        let mut old: HashMap<NodeId, BlockView> = views.drain(..).map(|v| (v.id, v)).collect();
        *views = ed
            .doc()
            .blocks
            .iter()
            .map(|b| old.remove(&b.id).unwrap_or_else(|| BlockView::new(b.id)))
            .collect();
    }

    pub(crate) fn zoom(&self) -> f64 {
        self.zoom.get()
    }

    /// Set the zoom (clamped); blocks re-render at the new scale and text
    /// rewraps to the window.
    pub(crate) fn set_zoom(&self, zoom: f64) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let old = self.zoom.replace(zoom);
        if (zoom - old).abs() < 1e-9 {
            return;
        }
        // Keep roughly the same part of the document in view.
        if let Some(adj) = self.vadjustment.borrow().as_ref() {
            adj.set_value(adj.value() * zoom / old);
        }
        self.obj().queue_allocate();
        self.obj().queue_draw();
        self.obj().emit_by_name::<()>("caret-moved", &[]);
    }

    pub(crate) fn zoom_by(&self, steps: f64) {
        self.set_zoom(self.zoom.get() * ZOOM_STEP.powf(steps));
    }

    /// Rounds widget coordinates to the nearest device pixel, taking the
    /// widget's position in the window surface into account (fractional
    /// scaling).
    fn pixel_snapper(&self) -> impl Fn(f64, f64) -> (f64, f64) + use<> {
        let obj = self.obj();
        let scale = self.device_scale();
        let origin = obj.native().and_then(|native| {
            let (sx, sy) = native.surface_transform();
            obj.compute_point(&native, &graphene::Point::new(0.0, 0.0))
                .map(|p| (p.x() as f64 + sx, p.y() as f64 + sy))
        });
        let (ox, oy) = origin.unwrap_or((0.0, 0.0));
        move |x, y| {
            let snap = |v: f64, o: f64| ((v + o) * scale).round() / scale - o;
            (snap(x, ox), snap(y, oy))
        }
    }

    fn device_scale(&self) -> f64 {
        let obj = self.obj();
        obj.native()
            .and_then(|n| n.surface())
            .map_or(obj.scale_factor() as f64, |s| s.scale())
    }

    fn scroll(&self) -> f64 {
        self.vadjustment
            .borrow()
            .as_ref()
            .map_or(0.0, |a| a.value())
    }

    /// Left edge of the text column in widget coordinates.
    fn page_x(&self, width: f64) -> f64 {
        let zoom = self.zoom.get();
        ((width - self.page_width_pt.get() * zoom) / 2.0).floor()
    }

    fn content_height(&self) -> f64 {
        let zoom = self.zoom.get();
        let views = self.views.borrow();
        let blocks: f64 = views
            .iter()
            .map(|v| (v.height_pt() + BLOCK_SPACING_PT) * zoom)
            .sum();
        blocks + 2.0 * TOP_MARGIN_PX
    }

    /// Top of every block in content coordinates (px, unscrolled).
    fn block_tops(&self) -> Vec<f64> {
        let zoom = self.zoom.get();
        let mut y = TOP_MARGIN_PX;
        self.views
            .borrow()
            .iter()
            .map(|v| {
                let top = y;
                y += (v.height_pt() + BLOCK_SPACING_PT) * zoom;
                top
            })
            .collect()
    }

    /// Caret stops of block `index`.
    fn block_stops(&self, index: usize) -> Vec<Stop> {
        let ed = self.editor.borrow();
        let views = self.views.borrow();
        match (ed.doc().blocks.get(index), views.get(index)) {
            (Some(block), Some(view)) => caret::stops(block, view.layout.as_ref()),
            _ => vec![],
        }
    }

    /// Block index and stop of the caret.
    fn caret_stop(&self) -> Option<(usize, Stop)> {
        let caret = self.editor.borrow().caret().clone();
        let index = self.editor.borrow().doc().block_index(caret.block)?;
        let stops = self.block_stops(index);
        let stop = caret::find(&stops, &caret)?.clone();
        Some((index, stop))
    }

    /// The caret in widget coordinates.
    fn caret_rect(&self) -> Option<graphene::Rect> {
        let zoom = self.zoom.get();
        let (index, stop) = self.caret_stop()?;
        let top = *self.block_tops().get(index)?;
        let x0 = self.page_x(self.obj().width() as f64);
        let x = (x0 + stop.x * zoom - CARET_WIDTH_PX / 2.0).round();
        let y = top + stop.top * zoom - self.scroll();
        let h = ((stop.bottom - stop.top) * zoom).max(8.0);
        Some(graphene::Rect::new(
            x as f32,
            y as f32,
            CARET_WIDTH_PX as f32,
            h as f32,
        ))
    }

    /// The caret position for a click at widget coordinates.
    fn locate(&self, x: f64, y: f64) -> Option<CursorPath> {
        let zoom = self.zoom.get();
        let cy = y + self.scroll();
        let tops = self.block_tops();
        // The gap below a block belongs to it.
        let index = tops.iter().rposition(|&t| t <= cy).unwrap_or(0);
        let top = *tops.get(index)?;
        let bx = (x - self.page_x(self.obj().width() as f64)) / zoom;
        let by = (cy - top) / zoom;
        let stops = self.block_stops(index);
        caret::hit(&stops, bx, by).map(|s| s.cursor.clone())
    }

    /// Up/down to the neighbouring line (across blocks), line start/end.
    fn geometric_move(&self, cmd: Command) -> Option<CursorPath> {
        let (index, current) = self.caret_stop()?;
        let stops = self.block_stops(index);
        let lines = caret::lines(&stops);
        let line = lines
            .iter()
            .position(|l| l.iter().any(|&i| stops[i].cursor == current.cursor))
            .or_else(|| {
                lines
                    .iter()
                    .position(|l| (stops[l[0]].line - current.line).abs() < 2.0)
            })?;
        let pick = |stops: &[Stop], line: &[usize], x: f64| {
            caret::nearest_in_line(stops, line, x).map(|s| s.cursor.clone())
        };
        match cmd {
            Command::LineStart | Command::LineEnd => {
                let end = matches!(cmd, Command::LineEnd);
                caret::line_edge(&stops, &lines[line], end).map(|s| s.cursor.clone())
            }
            Command::Up | Command::Down => {
                let x = self.goal_x.get().unwrap_or(current.x);
                self.goal_x.set(Some(x));
                let down = matches!(cmd, Command::Down);
                let target = if down { line + 1 } else { line.wrapping_sub(1) };
                if let Some(l) = lines.get(target) {
                    return pick(&stops, l, x);
                }
                let count = self.views.borrow().len();
                let next = if down {
                    index + 1
                } else {
                    index.checked_sub(1)?
                };
                if next >= count {
                    return None;
                }
                let stops = self.block_stops(next);
                let lines = caret::lines(&stops);
                let l = if down { lines.first() } else { lines.last() }?;
                pick(&stops, l, x)
            }
            _ => None,
        }
    }

    /// Scroll so the caret is visible.
    fn reveal_caret(&self) {
        let Some(r) = self.caret_rect() else {
            return;
        };
        let Some(adj) = self.vadjustment.borrow().clone() else {
            return;
        };
        let margin = 24.0;
        let (top, bottom) = (r.y() as f64 - margin, (r.y() + r.height()) as f64 + margin);
        let page = adj.page_size();
        let value = adj.value();
        if top < 0.0 {
            adj.set_value(value + top);
        } else if bottom > page {
            adj.set_value(value + bottom - page);
        }
    }

    fn update_im_location(&self) {
        if let (Some(im), Some(r)) = (self.im.get(), self.caret_rect()) {
            let rect = gdk::Rectangle::new(
                r.x() as i32,
                r.y() as i32,
                r.width().ceil() as i32,
                r.height() as i32,
            );
            im.set_cursor_location(&rect);
        }
    }

    fn restart_blink(&self) {
        self.stop_blink();
        self.caret_on.set(true);
        let weak = self.obj().downgrade();
        let id = glib::timeout_add_local(BLINK_INTERVAL, move || {
            let Some(obj) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let imp = obj.imp();
            imp.caret_on.set(!imp.caret_on.get());
            obj.queue_draw();
            glib::ControlFlow::Continue
        });
        *self.blink.borrow_mut() = Some(id);
        self.obj().queue_draw();
    }

    fn stop_blink(&self) {
        if let Some(id) = self.blink.borrow_mut().take() {
            id.remove();
        }
    }

    /// Submit compile jobs for blocks whose source changed, or for all
    /// blocks if `force` (render settings changed).
    fn submit_blocks(&self, force: bool) {
        if self.page_width_pt.get() <= 0.0 {
            return; // Not allocated yet; size_allocate will submit.
        }
        let ed = self.editor.borrow();
        let mut views = self.views.borrow_mut();
        for (block, view) in ed.doc().blocks.iter().zip(views.iter_mut()) {
            let (snippet, map) = serialize_block(block);
            if !force && snippet == view.snippet {
                continue;
            }
            let generation = self.generation.get() + 1;
            self.generation.set(generation);
            view.snippet = snippet.clone();
            view.submitted = generation;
            self.worker.get().expect("worker").submit(CompileJob {
                block: block.id,
                generation,
                snippet,
                map,
                width_pt: self.page_width_pt.get(),
                pixel_per_pt: self.pixel_per_pt.get(),
            });
        }
    }

    fn on_result(&self, result: CompileResult) {
        {
            let mut views = self.views.borrow_mut();
            let Some(view) = views.iter_mut().find(|v| v.id == result.block) else {
                return;
            };
            if result.generation < view.shown {
                return; // Older than what is on screen.
            }
            view.shown = result.generation;
            match result.outcome {
                Ok(rendered) => {
                    view.texture = Some(texture(rendered.raster));
                    view.texture_ppp = rendered.pixel_per_pt;
                    view.layout = Some(rendered.layout);
                    view.error = None;
                }
                Err(errors) => view.error = Some(errors.join("\n")),
            }
        }
        let obj = self.obj();
        obj.queue_resize();
        obj.queue_draw();
        if self.editor.borrow().caret().block == result.block {
            if self.reveal.take() {
                self.reveal_caret();
            }
            self.update_im_location();
        }
    }

    fn set_adjustment(
        &self,
        slot: &RefCell<Option<gtk::Adjustment>>,
        adj: Option<gtk::Adjustment>,
    ) {
        if let Some(adj) = &adj {
            let obj = self.obj().downgrade();
            adj.connect_value_changed(move |_| {
                if let Some(obj) = obj.upgrade() {
                    obj.queue_draw();
                }
            });
        }
        *slot.borrow_mut() = adj;
    }

    fn configure_adjustments(&self, width: f64, height: f64) {
        if let Some(v) = self.vadjustment.borrow().as_ref() {
            let upper = self.content_height().max(height);
            let value = v.value().clamp(0.0, upper - height);
            v.configure(value, 0.0, upper, 0.1 * height, 0.9 * height, height);
        }
        if let Some(h) = self.hadjustment.borrow().as_ref() {
            h.configure(0.0, 0.0, width, 0.1 * width, 0.9 * width, width);
        }
    }
}

/// The command for a key press, if any. Printable keys go through the input
/// method and arrive as commits instead.
fn command_for(key: gdk::Key, state: gdk::ModifierType) -> Option<Command> {
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    use gdk::Key;
    Some(match key {
        Key::Left | Key::KP_Left => Command::Left,
        Key::Right | Key::KP_Right => Command::Right,
        Key::Up | Key::KP_Up => Command::Up,
        Key::Down | Key::KP_Down => Command::Down,
        Key::Home | Key::KP_Home if ctrl => Command::DocStart,
        Key::End | Key::KP_End if ctrl => Command::DocEnd,
        Key::Home | Key::KP_Home => Command::LineStart,
        Key::End | Key::KP_End => Command::LineEnd,
        Key::BackSpace => Command::Backspace,
        Key::Delete | Key::KP_Delete => Command::Delete,
        Key::Return | Key::KP_Enter | Key::ISO_Enter => Command::Enter,
        Key::Escape => Command::Escape,
        Key::ISO_Left_Tab => Command::PrevSlot,
        Key::Tab | Key::KP_Tab if shift => Command::PrevSlot,
        Key::Tab | Key::KP_Tab => Command::NextSlot,
        _ if ctrl && key.to_unicode().is_some_and(|c| ('0'..='6').contains(&c)) => {
            let digit = key.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(0) as u8;
            Command::Heading((digit > 0).then_some(digit))
        }
        Key::plus | Key::equal | Key::KP_Add if ctrl => Command::Zoom(1.0),
        Key::minus | Key::underscore | Key::KP_Subtract if ctrl => Command::Zoom(-1.0),
        _ if ctrl => match key.to_lower() {
            Key::z if shift => Command::Redo,
            Key::z => Command::Undo,
            Key::y => Command::Redo,
            _ => return None,
        },
        _ => return None,
    })
}

/// Wraps a shared raster so GLib can borrow its pixels without a copy.
struct RasterBytes(Arc<Raster>);

impl AsRef<[u8]> for RasterBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0.data
    }
}

fn texture(raster: Arc<Raster>) -> gdk::Texture {
    let (w, h) = (raster.width as i32, raster.height as i32);
    let stride = raster.width as usize * 4;
    let bytes = glib::Bytes::from_owned(RasterBytes(raster));
    gdk::MemoryTexture::new(
        w,
        h,
        gdk::MemoryFormat::R8g8b8a8Premultiplied,
        &bytes,
        stride,
    )
    .upcast()
}
