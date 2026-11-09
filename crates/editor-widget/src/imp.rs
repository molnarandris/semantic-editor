use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gtk::glib::subclass::Signal;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use model::{Block, CursorPath, Document, Editor, NodeId};
use typst_bridge::{
    Around, BLOCK_PAD_PT, BlockLayout, CompileJob, CompileResult, CounterState, HEADER_SPECIMEN,
    Raster, SpanMap, Worker, serialize_block, serialize_math_node,
};

use crate::caret::{self, Stop};
use crate::{PreviewTarget, jobs};

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
    /// Code around, snippet and span map of the last submitted job, to skip
    /// redundant compiles.
    around: Around,
    snippet: String,
    map: SpanMap,
    /// Counter-sensitive blocks: the counters at the start of the last
    /// submitted job, and at its end once its result arrived.
    start: Option<CounterState>,
    end: Option<CounterState>,
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
    /// Caret stops from the block and its layout; reset when either changes.
    stops: OnceCell<Rc<[Stop]>>,
}

impl BlockView {
    fn new(id: NodeId) -> Self {
        Self {
            id,
            around: Around::default(),
            snippet: String::new(),
            map: SpanMap::default(),
            start: None,
            end: None,
            submitted: 0,
            shown: 0,
            texture: None,
            texture_ppp: 1.0,
            layout: None,
            error: None,
            stops: OnceCell::new(),
        }
    }

    fn height_pt(&self) -> f64 {
        self.layout
            .as_ref()
            .map_or(0.0, |l| l.height)
            .max(MIN_BLOCK_HEIGHT_PT)
    }
}

/// Job id of preview renders (never a real node).
const PREVIEW_ID: NodeId = NodeId(u64::MAX);
/// Largest device pixels per point a preview is rendered at.
const MAX_PREVIEW_PPP: f64 = 12.0;

/// A zoomed render for the source editor's preview, cropped to its ink.
#[derive(Default)]
struct Preview {
    target: Option<PreviewTarget>,
    pixel_per_pt: f64,
    /// Code around, snippet, page width and scale of the last submitted job.
    submitted: Option<(Around, String, f64, f64)>,
    /// Generation of the last submitted job and of the result shown.
    generation: u64,
    shown: u64,
    texture: Option<(gdk::Texture, f64)>,
    /// Message of the last failed compile; the last good render stays.
    error: Option<String>,
}

#[derive(Default)]
pub struct SemEditor {
    pub(crate) editor: RefCell<Editor>,
    views: RefCell<Vec<BlockView>>,
    preview: RefCell<Preview>,
    worker: OnceCell<Worker>,
    generation: Cell<u64>,
    /// Page width and pixels per point the current renders were requested at.
    page_width_pt: Cell<f64>,
    pixel_per_pt: Cell<f64>,
    /// Logical pixels per point.
    zoom: Cell<f64>,
    /// Show code blocks' source instead of a marker.
    show_code: Cell<bool>,
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
        SIGNALS.get_or_init(|| {
            vec![
                Signal::builder("caret-moved").build(),
                // A raw/diagram block's source should be opened for editing
                // (Ctrl+Enter or double click on it). Param: block id.
                Signal::builder("edit-source")
                    .param_types([u64::static_type()])
                    .build(),
                // The preview render (see `set_preview`) changed.
                Signal::builder("preview-rendered").build(),
            ]
        })
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
        let disk_cache = glib::user_cache_dir()
            .join("semantic-editor")
            .join("renders");
        let worker = Worker::spawn(Some(disk_cache), move |result| {
            let weak = weak.clone();
            glib::MainContext::default().invoke(move || {
                if let Some(editor) = weak.upgrade() {
                    editor.imp().on_result(result);
                }
            });
        });
        let _ = self.worker.set(worker);

        let mut editor = self.editor.borrow_mut();
        editor.set_symbol_resolver(typst_bridge::resolve_symbol);
        editor.set_call_resolver(typst_bridge::call_shape);
        editor.set_math_call_resolver(typst_bridge::math_call_shape);
        drop(editor);
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
            self.snapshot_env_outline(snapshot, x0, scroll);
            self.snapshot_call_outline(snapshot, x0, scroll);
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
                    // pixel grid, as a plain texture node: a scaled-texture
                    // node resamples (blurs) even at exactly 1:1.
                    let pad = BLOCK_PAD_PT * zoom;
                    let (px, py) = snap(x0 - pad, y - pad);
                    let per_px = zoom / view.texture_ppp;
                    let rect = graphene::Rect::new(
                        px as f32,
                        py as f32,
                        (texture.width() as f64 * per_px) as f32,
                        (texture.height() as f64 * per_px) as f32,
                    );
                    snapshot.append_texture(texture, &rect);
                }
                if view.error.is_some() {
                    let marker =
                        graphene::Rect::new((x0 - 14.0) as f32, (y + 5.0) as f32, 6.0, 6.0);
                    snapshot.append_color(&gdk::RGBA::new(0.88, 0.11, 0.14, 1.0), &marker);
                }
            }
            y += h + BLOCK_SPACING_PT * zoom;
        }

        if obj.has_focus() {
            self.snapshot_formula_marks(snapshot, x0, scroll);
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
    /// Ctrl+Enter: open the caret's raw/diagram block in the source dialog.
    EditSource,
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
        editor.set_call_resolver(typst_bridge::call_shape);
        editor.set_math_call_resolver(typst_bridge::math_call_shape);
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
        click.connect_pressed(move |_, n_press, x, y| {
            if let Some(obj) = weak.upgrade() {
                obj.grab_focus();
                let imp = obj.imp();
                if let Some(env) = imp.heading_at(x, y)
                    && imp.editor.borrow_mut().edit_title(env)
                {
                    // An environment's heading: edit (or add) its title
                    // (not for a function that takes none).
                    imp.goal_x.set(None);
                    imp.after_command(true);
                } else if let Some(cursor) = imp.locate(x, y) {
                    imp.editor.borrow_mut().set_caret(cursor);
                    imp.goal_x.set(None);
                    imp.after_command(false);
                    if n_press == 2 {
                        imp.request_source_edit();
                    }
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
        if !matches!(cmd, Command::Up | Command::Down) {
            self.goal_x.set(None);
        }
        // Commands report whether they changed anything, but the view syncs
        // by comparing sources anyway, so the results are not needed.
        let mut ed = self.editor.borrow_mut();
        match cmd {
            Command::Left => _ = ed.move_left(),
            Command::Right => _ = ed.move_right(),
            Command::Backspace => _ = ed.backspace(),
            Command::Delete => _ = ed.delete(),
            Command::EditSource => {
                drop(ed);
                self.request_source_edit();
                return;
            }
            Command::Enter => _ = ed.enter(),
            Command::Undo => _ = ed.undo(),
            Command::Redo => _ = ed.redo(),
            Command::Escape => _ = ed.escape(),
            Command::NextSlot => _ = ed.next_slot(false),
            Command::PrevSlot => _ = ed.next_slot(true),
            Command::Heading(level) => _ = ed.set_heading(level),
            Command::DocStart => {
                let c = ed.doc().doc_start();
                ed.set_caret(c);
            }
            Command::DocEnd => {
                let c = ed.doc().doc_end();
                ed.set_caret(c);
            }
            Command::Zoom(steps) => {
                drop(ed);
                self.zoom_by(steps);
                return;
            }
            // Moves that need the rendered geometry.
            Command::Up | Command::Down | Command::LineStart | Command::LineEnd => {
                let down = matches!(cmd, Command::Down);
                let target = match cmd {
                    Command::Up | Command::Down => ed.vertical_target(down),
                    _ => None,
                };
                drop(ed);
                let next = match target {
                    Some(t) => Some(self.column_in_row(t)),
                    None => self.geometric_move(cmd),
                };
                ed = self.editor.borrow_mut();
                if let Some(c) = next {
                    ed.set_caret(c);
                }
            }
        }
        drop(ed);
        self.after_command(true);
    }

    /// Bring the view in line with the model after a command. Any command
    /// may change the document (moving the caret finishes a word, `chi` →
    /// χ); only blocks whose source changed are recompiled.
    pub(crate) fn after_command(&self, reveal: bool) {
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

    /// Ask the app to open the caret's raw/diagram block, or the code atom
    /// next to it, for editing.
    fn request_source_edit(&self) {
        let target = self.editor.borrow().source_target();
        if let Some(id) = target {
            self.obj().emit_by_name::<()>("edit-source", &[&id.0]);
        }
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
    /// A faint outline around the environment the caret is in (tinted) or
    /// just before/after (outline only), so in and out can be told apart.
    fn snapshot_env_outline(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let ed = self.editor.borrow();
        let Some((env, inside)) = ed.env_focus() else {
            return;
        };
        let Some(index) = ed.doc().top_index(env) else {
            return;
        };
        let Some(&top) = self.block_tops().get(index) else {
            return;
        };
        let views = self.views.borrow();
        let Some(view) = views.get(index) else {
            return;
        };
        let Some(layout) = view.layout.as_ref() else {
            return;
        };
        let r = if view.id == env {
            typst_bridge::Rect {
                x: 0.0,
                y: 0.0,
                w: layout.width,
                h: view.height_pt(),
            }
        } else {
            let Some(r) = layout.box_of(env) else { return };
            r
        };
        let pad = caret::OUTLINE_PAD_PT * zoom;
        let rect = graphene::Rect::new(
            (x0 + r.x * zoom - pad) as f32,
            (top + r.y * zoom - scroll - pad) as f32,
            (r.w * zoom + 2.0 * pad) as f32,
            (r.h * zoom + 2.0 * pad) as f32,
        );
        let blue = |a: f32| gdk::RGBA::new(0.21, 0.52, 0.89, a);
        let outline = gsk::RoundedRect::from_rect(rect, 3.0);
        if inside {
            snapshot.push_rounded_clip(&outline);
            snapshot.append_color(&blue(0.04), &rect);
            snapshot.pop();
        }
        // Marked by Backspace (the next one unwraps it): stronger.
        let (width, alpha) = if ed.env_marked() {
            (2.0, 0.8)
        } else {
            (1.0, 0.35)
        };
        snapshot.append_border(&outline, &[width; 4], &[blue(alpha); 4]);
    }

    /// The inline call the caret is in (bold, italic, `#name[…]`): tinted
    /// and outlined like an environment, one box per line it spans.
    fn snapshot_call_outline(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let (block, call) = {
            let ed = self.editor.borrow();
            let Some(call) = ed.call_focus() else {
                return;
            };
            (ed.caret().block, call)
        };
        let Some(index) = self.editor.borrow().doc().top_index(block) else {
            return;
        };
        let Some(&top) = self.block_tops().get(index) else {
            return;
        };
        let stops = self.block_stops(index);
        let pad = 2.0 * zoom;
        let blue = |a: f32| gdk::RGBA::new(0.21, 0.52, 0.89, a);
        for r in caret::call_outline(&stops, block, &call) {
            let rect = graphene::Rect::new(
                (x0 + r.x * zoom - pad) as f32,
                (top + r.y * zoom - scroll - pad) as f32,
                (r.w * zoom + 2.0 * pad) as f32,
                (r.h * zoom + 2.0 * pad) as f32,
            );
            let outline = gsk::RoundedRect::from_rect(rect, 2.0);
            snapshot.append_color(&blue(0.04), &rect);
            snapshot.append_border(&outline, &[1.0; 4], &[blue(0.35); 4]);
        }
    }

    /// The alignment points and line breaks of the formula being edited,
    /// which the render does not show: a thin line at `&`, `↵` at the end
    /// of a line.
    fn snapshot_formula_marks(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let caret = self.editor.borrow().caret().clone();
        let Some(index) = self.editor.borrow().doc().top_index(caret.block) else {
            return;
        };
        let Some(&top) = self.block_tops().get(index) else {
            return;
        };
        let stops = self.block_stops(index);
        let marks = caret::formula_marks(self.editor.borrow().doc(), &caret, &stops);
        let color = gdk::RGBA::new(0.42, 0.39, 0.56, 0.55);
        for mark in marks {
            match mark {
                caret::Mark::Align { x, top: t, bottom } => {
                    let r = graphene::Rect::new(
                        (x0 + x * zoom - 0.5).round() as f32,
                        (top + t * zoom - scroll) as f32,
                        1.0,
                        ((bottom - t) * zoom) as f32,
                    );
                    snapshot.append_color(&color, &r);
                }
                caret::Mark::Break { x, top: t, bottom } => {
                    let layout = self.obj().create_pango_layout(Some("↵"));
                    let mut font = gtk::pango::FontDescription::new();
                    font.set_absolute_size((bottom - t) * zoom * 0.6 * gtk::pango::SCALE as f64);
                    layout.set_font_description(Some(&font));
                    let (_, h) = layout.pixel_size();
                    let y = top + (t + bottom) / 2.0 * zoom - scroll - h as f64 / 2.0;
                    snapshot.save();
                    snapshot.translate(&graphene::Point::new(
                        (x0 + x * zoom + 2.0) as f32,
                        y as f32,
                    ));
                    snapshot.append_layout(&layout, &color);
                    snapshot.restore();
                }
            }
        }
    }

    fn snapshot_highlights(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let ed = self.editor.borrow();
        let (context, selected) = (ed.context_ids(), ed.selected_ids());
        if context.is_empty() && selected.is_empty() {
            return;
        }
        let Some(index) = ed.doc().top_index(ed.caret().block) else {
            return;
        };
        let Some(&top) = self.block_tops().get(index) else {
            return;
        };
        let views = self.views.borrow();
        let Some(layout) = views.get(index).and_then(|v| v.layout.as_ref()) else {
            return;
        };
        // A whole block (selected diagram) covers its full area.
        let whole = |id: NodeId| {
            (views[index].id == id).then_some(typst_bridge::Rect {
                x: 0.0,
                y: 0.0,
                w: layout.width,
                h: views[index].height_pt(),
            })
        };
        let rect = |id: NodeId, pad: f64| {
            whole(id).or_else(|| layout.box_of(id)).map(|r| {
                graphene::Rect::new(
                    (x0 + r.x * zoom - pad) as f32,
                    (top + r.y * zoom - scroll - pad) as f32,
                    (r.w * zoom + 2.0 * pad) as f32,
                    (r.h * zoom + 2.0 * pad) as f32,
                )
            })
        };
        let blue = |a: f32| gdk::RGBA::new(0.21, 0.52, 0.89, a);
        // A display equation's shading spans its whole line (the page, or
        // the environment it is in), so it stays symmetric with a number.
        let line = |r: graphene::Rect| {
            let block = ed.caret().block;
            if !matches!(
                ed.doc().block(block).map(|b| &b.kind),
                Some(Block::DisplayMath(_))
            ) {
                return r;
            }
            let (x, w) = match ed.doc().place(block).and_then(|p| p.parent) {
                Some(env) => match layout.box_of(env) {
                    Some(e) => (e.x, e.w),
                    None => return r,
                },
                None => (0.0, layout.width),
            };
            graphene::Rect::new((x0 + x * zoom) as f32, r.y(), (w * zoom) as f32, r.height())
        };
        if let Some(r) = context.first().and_then(|&id| rect(id, 2.0)) {
            snapshot.append_color(&blue(0.07), &line(r));
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

    /// Show code blocks' source (`true`) or a marker. Only code blocks
    /// re-render.
    pub(crate) fn set_show_code(&self, show: bool) {
        if self.show_code.replace(show) != show {
            self.submit_blocks(false);
        }
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

    /// Caret stops of block `index` (computed once per block and layout).
    fn block_stops(&self, index: usize) -> Rc<[Stop]> {
        let ed = self.editor.borrow();
        let views = self.views.borrow();
        match (ed.doc().blocks.get(index), views.get(index)) {
            (Some(block), Some(view)) => view
                .stops
                .get_or_init(|| caret::stops(block, view.layout.as_ref()).into())
                .clone(),
            _ => Rc::from([]),
        }
    }

    /// Block index and stop of the caret.
    fn caret_stop(&self) -> Option<(usize, Stop)> {
        let caret = self.editor.borrow().caret().clone();
        // Views are per top-level block; an environment's view has the
        // stops of the blocks inside it.
        let index = self.editor.borrow().doc().top_index(caret.block)?;
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
    /// The block under widget point `(x, y)`, its stops and the point in
    /// its coordinates (pt).
    fn block_at(&self, x: f64, y: f64) -> Option<(Rc<[Stop]>, f64, f64)> {
        let zoom = self.zoom.get();
        let cy = y + self.scroll();
        let tops = self.block_tops();
        // The gap below a block belongs to it.
        let index = tops.iter().rposition(|&t| t <= cy).unwrap_or(0);
        let top = *tops.get(index)?;
        let bx = (x - self.page_x(self.obj().width() as f64)) / zoom;
        let by = (cy - top) / zoom;
        Some((self.block_stops(index), bx, by))
    }

    fn locate(&self, x: f64, y: f64) -> Option<CursorPath> {
        let (stops, bx, by) = self.block_at(x, y)?;
        caret::hit(&stops, bx, by).map(|s| s.cursor.clone())
    }

    fn heading_at(&self, x: f64, y: f64) -> Option<NodeId> {
        let (stops, bx, by) = self.block_at(x, y)?;
        caret::heading_at(&stops, bx, by)
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
        if force {
            // Forget what was submitted: every block is stale, also those
            // that wait for the counter chain and are submitted later.
            for view in views.iter_mut() {
                view.snippet.clear();
            }
        }
        let jobs = {
            let submitted: Vec<_> = views
                .iter()
                .map(|v| jobs::Submitted {
                    around: &v.around,
                    snippet: &v.snippet,
                    map: &v.map,
                    end: v.end.as_ref(),
                })
                .collect();
            jobs::stale_blocks(
                &ed.doc().blocks,
                &ed.doc().header,
                self.show_code.get(),
                &submitted,
            )
        };
        for job in jobs {
            let (block, view) = (&ed.doc().blocks[job.index], &mut views[job.index]);
            let generation = self.generation.get() + 1;
            self.generation.set(generation);
            view.around = job.around.clone();
            view.snippet = job.snippet.clone();
            view.map = job.map.clone();
            view.start = job.start;
            view.end = None;
            view.submitted = generation;
            // The block changed; its stops follow the new tree right away
            // (against the old layout until the new one arrives).
            view.stops = OnceCell::new();
            self.worker.get().expect("worker").submit(CompileJob {
                block: block.id,
                generation,
                around: job.around,
                snippet: job.snippet,
                map: job.map,
                width_pt: self.page_width_pt.get(),
                pixel_per_pt: self.pixel_per_pt.get(),
                persist: job.persist,
                transient: false,
            });
        }
        drop((ed, views));
        self.submit_preview();
    }

    /// Render `target` for a preview at `pixel_per_pt` (device pixels per
    /// point), kept up to date with its source; `None` stops it.
    pub(crate) fn set_preview(&self, target: Option<(PreviewTarget, f64)>) {
        {
            let mut preview = self.preview.borrow_mut();
            match target {
                None => *preview = Preview::default(),
                Some((target, ppp)) => {
                    if preview.target != Some(target) {
                        *preview = Preview::default();
                        preview.target = Some(target);
                    }
                    preview.pixel_per_pt = ppp.clamp(0.25, MAX_PREVIEW_PPP);
                }
            }
        }
        self.submit_preview();
    }

    pub(crate) fn preview_texture(&self) -> Option<(gdk::Texture, f64)> {
        self.preview.borrow().texture.clone()
    }

    pub(crate) fn preview_error(&self) -> Option<String> {
        self.preview.borrow().error.clone()
    }

    /// Submit the preview job if its source, size or scale changed. A block
    /// is previewed as it was last submitted for the page; the header with
    /// a specimen of text.
    fn submit_preview(&self) {
        let width = self.page_width_pt.get();
        let mut preview = self.preview.borrow_mut();
        let Some(target) = preview.target else { return };
        let (around, snippet, map) = match target {
            PreviewTarget::Block(id) => {
                let ed = self.editor.borrow();
                let doc = ed.doc();
                let Some(top) = doc.top_block(id) else {
                    return;
                };
                let views = self.views.borrow();
                let Some(view) = views.iter().find(|v| v.id == top) else {
                    return;
                };
                if top == id {
                    (view.around.clone(), view.snippet.clone(), view.map.clone())
                } else {
                    // A block nested in an environment or list item, or a
                    // diagram in a formula: only it, with the definitions
                    // in force where it is.
                    let snippet = match doc.block(id) {
                        Some(block) => serialize_block(block).0,
                        None => match doc.find_node(id) {
                            Some((row, k)) => serialize_math_node(&row.nodes[k]),
                            None => return,
                        },
                    };
                    let around = Around {
                        header: view.around.header.clone(),
                        ..Around::default()
                    };
                    (around, snippet, SpanMap::default())
                }
            }
            PreviewTarget::Header => {
                let around = Around {
                    header: self.editor.borrow().doc().header.as_str().into(),
                    ..Around::default()
                };
                (around, HEADER_SPECIMEN.to_string(), SpanMap::default())
            }
        };
        if width <= 0.0 || snippet.is_empty() {
            return;
        }
        let key = (around.clone(), snippet.clone(), width, preview.pixel_per_pt);
        if preview.submitted.as_ref() == Some(&key) {
            return;
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        preview.generation = generation;
        preview.submitted = Some(key);
        self.worker.get().expect("worker").submit(CompileJob {
            block: PREVIEW_ID,
            generation,
            around,
            snippet,
            map,
            width_pt: width,
            pixel_per_pt: preview.pixel_per_pt,
            persist: false,
            transient: true,
        });
    }

    fn on_preview_result(&self, result: CompileResult) {
        {
            let mut preview = self.preview.borrow_mut();
            if preview.target.is_none() || result.generation < preview.shown {
                return;
            }
            preview.shown = result.generation;
            let rendered = match result.outcome {
                Ok(rendered) => rendered,
                Err(errors) => {
                    preview.error = Some(errors.join("\n"));
                    drop(preview);
                    self.obj().emit_by_name::<()>("preview-rendered", &[]);
                    return;
                }
            };
            preview.error = None;
            let raster = match rendered.raster.ink_bounds() {
                Some(bounds) => Arc::new(rendered.raster.crop(bounds)),
                None => rendered.raster,
            };
            preview.texture = Some((texture(raster), rendered.pixel_per_pt));
        }
        self.obj().emit_by_name::<()>("preview-rendered", &[]);
    }

    fn on_result(&self, result: CompileResult) {
        if result.block == PREVIEW_ID {
            self.on_preview_result(result);
            return;
        }
        let chained = {
            let mut views = self.views.borrow_mut();
            let Some(view) = views.iter_mut().find(|v| v.id == result.block) else {
                return;
            };
            if result.generation < view.shown {
                return; // Older than what is on screen.
            }
            view.shown = result.generation;
            view.stops = OnceCell::new();
            let end = match result.outcome {
                Ok(rendered) => {
                    view.texture = Some(texture(rendered.raster));
                    view.texture_ppp = rendered.pixel_per_pt;
                    view.layout = Some(rendered.layout);
                    view.error = None;
                    rendered.end_state
                }
                Err(errors) => {
                    view.error = Some(errors.join("\n"));
                    None
                }
            };
            // The counters this block ends with; a failed block passes its
            // start on unchanged.
            let current = result.generation == view.submitted && view.start.is_some();
            if current {
                view.end = end.or_else(|| view.start.clone());
            }
            current
        };
        if chained {
            // The next counter-sensitive block can go now.
            self.submit_blocks(false);
        }
        let obj = self.obj();
        obj.queue_resize();
        obj.queue_draw();
        let caret_view = {
            let ed = self.editor.borrow();
            let doc = ed.doc();
            doc.top_index(ed.caret().block).map(|i| doc.blocks[i].id)
        };
        if caret_view == Some(result.block) {
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
        Key::Return | Key::KP_Enter | Key::ISO_Enter if ctrl => Command::EditSource,
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
