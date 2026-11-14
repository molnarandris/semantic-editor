use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gtk::glib::subclass::Signal;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use model::{Block, BlockNode, CursorPath, Document, Editor, NodeId};
use typst_bridge::{
    Around, BLOCK_PAD_PT, BlockLayout, CompileJob, CompileResult, CounterState, HEADER_SPECIMEN,
    LabelTarget, Raster, SpanMap, Worker, outline_entry_rule, serialize_block, serialize_math_node,
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
    /// What the labels the block defines point to, from its latest render
    /// (kept while a new one compiles).
    targets: Vec<LabelTarget>,
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
            targets: vec![],
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

/// Job ids of outline entries: the heading's id with this bit set.
const OUTLINE_BIT: u64 = 1 << 62;
/// Outline entries: their page width in pt, and logical pixels per pt.
const OUTLINE_WIDTH_PT: f64 = 180.0;
const OUTLINE_PX_PER_PT: f64 = 1.25;

/// A heading rendered as an outline entry (`outline_entry_rule`).
#[derive(Default)]
struct OutlineEntry {
    /// Code around, snippet and pixels per point of the last submitted job.
    submitted: Option<(Around, String, f64)>,
    generation: u64,
    /// Cropped to the ink, with the device pixels per logical pixel.
    texture: Option<(gdk::Texture, f64)>,
}

/// The math call whose options tab shows (see `SemEditor::tab_call`).
type TabCall = (Rc<[Stop]>, f64, NodeId, Vec<model::Step>);

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
    outline: RefCell<HashMap<NodeId, OutlineEntry>>,
    worker: OnceCell<Worker>,
    generation: Cell<u64>,
    /// Page width and pixels per point the current renders were requested at.
    page_width_pt: Cell<f64>,
    pixel_per_pt: Cell<f64>,
    /// Logical pixels per point.
    zoom: Cell<f64>,
    /// Show code blocks' source instead of a marker.
    show_code: Cell<bool>,
    /// The top-level block holding the caret, as it was when renders were
    /// last requested: an edit in it shows as a change.
    caret_block: RefCell<Option<BlockNode>>,
    /// The top-level block being typed in (edited, caret still in it): its
    /// lines are broken greedily so they do not jump (`jobs::GREEDY_LINES`).
    editing: Cell<Option<NodeId>>,
    /// Zoom when a pinch gesture began.
    pinch_start: Cell<f64>,

    im: OnceCell<gtk::IMMulticontext>,
    caret_on: Cell<bool>,
    blink: RefCell<Option<glib::SourceId>>,
    /// Column kept while moving up and down, in pt.
    goal_x: Cell<Option<f64>>,
    /// Scroll the caret into view when its block is rendered.
    reveal: Cell<bool>,
    /// The caret jumped (to a heading, a reference's target, back): the
    /// next reveal puts it a third down the view.
    reveal_third: Cell<bool>,
    /// The completion list while a reference is typed: the chosen entry,
    /// the label typed when it was chosen (a new one starts at the top),
    /// and the rows as last drawn (widget coordinates), for clicks.
    completion: Cell<usize>,
    completion_typed: RefCell<String>,
    completion_rows: RefCell<Vec<(graphene::Rect, String)>>,

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
                // A heading's outline entry was rendered. Param: its id.
                Signal::builder("outline-rendered")
                    .param_types([u64::static_type()])
                    .build(),
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
        self.snapshot_ref_marks(snapshot, x0, scroll);
        self.snapshot_remarks(snapshot, x0, scroll);
        if obj.has_focus() {
            self.snapshot_env_outline(snapshot, x0, scroll);
            self.snapshot_call_outline(snapshot, x0, scroll);
            self.snapshot_math_call_outline(snapshot, x0, scroll);
            self.snapshot_highlights(snapshot, x0, scroll);
            self.snapshot_selection(snapshot, x0, scroll);
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

        self.snapshot_label_pills(snapshot, x0, scroll, obj.has_focus());
        if obj.has_focus() {
            self.snapshot_formula_marks(snapshot, x0, scroll);
            self.snapshot_call_tab(snapshot, x0, scroll);
        }
        if obj.has_focus()
            && self.caret_on.get()
            && let Some(r) = self.caret_rect()
        {
            snapshot.append_color(&gdk::RGBA::new(0.1, 0.1, 0.12, 1.0), &r);
        }
        if obj.has_focus() {
            self.snapshot_completions(snapshot);
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
    /// Shift+Enter: a line break.
    LineBreak,
    /// Alt+←: back to before the last reference followed.
    Back,
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
    /// Ctrl+C / Ctrl+X: the selection to the clipboard as Typst.
    Copy,
    Cut,
    /// Ctrl+V: the clipboard's text, read as Typst, at the caret.
    Paste,
}

impl Command {
    /// A caret move: with Shift it extends the selection.
    fn is_move(&self) -> bool {
        matches!(
            self,
            Command::Left
                | Command::Right
                | Command::Up
                | Command::Down
                | Command::LineStart
                | Command::LineEnd
                | Command::DocStart
                | Command::DocEnd
        )
    }
}

impl SemEditor {
    pub(crate) fn set_document(&self, doc: Document) {
        let mut editor = Editor::new(doc);
        editor.set_read_only(self.editor.borrow().read_only());
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
            if obj.imp().completion_key(key) {
                return glib::Propagation::Stop;
            }
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            match command_for(key, state) {
                Some(cmd) if shift && cmd.is_move() => {
                    obj.imp().run_selecting(cmd);
                    glib::Propagation::Stop
                }
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

        // A remark's text when the pointer is on its shading or bar.
        obj.set_has_tooltip(true);
        obj.connect_query_tooltip(|obj, x, y, _keyboard, tooltip| {
            let Some(text) = obj.imp().remark_at(x as f64, y as f64) else {
                return false;
            };
            tooltip.set_text(Some(&text));
            true
        });

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let weak = obj.downgrade();
        click.connect_pressed(move |gesture, n_press, x, y| {
            if let Some(obj) = weak.upgrade() {
                obj.grab_focus();
                let imp = obj.imp();
                let ctrl = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                if let Some(label) = imp.completion_at(x, y) {
                    imp.choose_completion(&label);
                } else if ctrl
                    && let Some(label) = imp.ref_at(x, y)
                    && imp.editor.borrow_mut().follow_ref(&label)
                {
                    // Ctrl+click on a reference: to its target (Alt+←
                    // comes back).
                    imp.goal_x.set(None);
                    imp.after_jump();
                } else if let Some(env) = imp.heading_at(x, y)
                    && imp.editor.borrow_mut().edit_title(env)
                {
                    // An environment's heading: edit (or add) its title
                    // (not for a function that takes none).
                    imp.goal_x.set(None);
                    imp.after_command(true);
                } else if let Some(cursor) = imp.locate(x, y) {
                    let shift = gesture
                        .current_event_state()
                        .contains(gdk::ModifierType::SHIFT_MASK);
                    let mut ed = imp.editor.borrow_mut();
                    let anchor = ed.anchor();
                    ed.set_caret(cursor);
                    if shift {
                        ed.set_anchor(anchor);
                    }
                    drop(ed);
                    imp.goal_x.set(None);
                    imp.after_command(false);
                    if n_press == 2 {
                        imp.request_source_edit();
                    }
                }
            }
        });
        obj.add_controller(click);

        // Dragging selects from where the button went down.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = obj.downgrade();
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(obj) = weak.upgrade() else {
                return;
            };
            let Some((x, y)) = gesture.start_point() else {
                return;
            };
            let imp = obj.imp();
            let (Some(anchor), Some(head)) = (imp.locate(x, y), imp.locate(x + dx, y + dy)) else {
                return;
            };
            let mut ed = imp.editor.borrow_mut();
            if head == ed.caret().clone() && ed.selection().is_some() {
                return;
            }
            ed.set_caret(head);
            ed.set_anchor(anchor);
            drop(ed);
            imp.goal_x.set(None);
            imp.after_command(true);
        });
        obj.add_controller(drag);

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

    /// Shift + a move: the caret moves, the selection's anchor stays.
    fn run_selecting(&self, cmd: Command) {
        let anchor = self.editor.borrow().anchor();
        self.run(cmd);
        self.editor.borrow_mut().set_anchor(anchor);
        self.obj().queue_draw();
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
            Command::LineBreak => _ = ed.line_break(),
            Command::Back => self.reveal_third.set(ed.go_back()),
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
            Command::Paste => {
                drop(ed);
                let weak = self.obj().downgrade();
                self.obj().clipboard().read_text_async(
                    None::<&gtk::gio::Cancellable>,
                    move |text| {
                        let (Some(obj), Ok(Some(text))) = (weak.upgrade(), text) else {
                            return;
                        };
                        let imp = obj.imp();
                        let in_math = imp.editor.borrow().in_math();
                        let fragment = typst_bridge::parse_fragment(&text, in_math);
                        imp.editor.borrow_mut().paste(fragment);
                        imp.goal_x.set(None);
                        imp.after_command(true);
                    },
                );
                return;
            }
            Command::Copy | Command::Cut => {
                let Some(fragment) = ed.selection_fragment() else {
                    return;
                };
                self.obj()
                    .clipboard()
                    .set_text(&typst_bridge::fragment_source(&fragment));
                if matches!(cmd, Command::Cut) {
                    ed.backspace();
                }
            }
            // Moves that need the rendered geometry.
            Command::Up | Command::Down | Command::LineStart | Command::LineEnd => {
                let down = matches!(cmd, Command::Down);
                let target = match cmd {
                    Command::Up | Command::Down => ed.vertical_target(down),
                    _ => ed.tab_edge(matches!(cmd, Command::LineEnd)),
                };
                drop(ed);
                let next = match target {
                    Some(t) if down || matches!(cmd, Command::Up) => Some(self.column_in_row(t)),
                    Some(t) => Some(t),
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

    /// After the caret jumped somewhere: as `after_command`, with the caret
    /// shown a third down the view.
    pub(crate) fn after_jump(&self) {
        self.reveal_third.set(true);
        self.after_command(true);
    }

    /// Ask the app to open the caret's raw/diagram block, or the code atom
    /// next to it, for editing.
    fn request_source_edit(&self) {
        let target = self.editor.borrow().source_target();
        if let Some(id) = target {
            self.obj().emit_by_name::<()>("edit-source", &[&id.0]);
        }
    }

    /// Problems in document order, one entry per top-level block: blocks
    /// that failed to compile (first line of the error), then problems
    /// with labels and references.
    pub(crate) fn problems(&self) -> Vec<(NodeId, String)> {
        let ed = self.editor.borrow();
        let blocks = &ed.doc().blocks;
        let views = self.views.borrow();
        let mut found: Vec<(usize, String)> = views
            .iter()
            .filter_map(|v| {
                let error = v.error.as_ref()?;
                let i = blocks.iter().position(|b| b.id == v.id)?;
                Some((i, error.lines().next().unwrap_or("").to_string()))
            })
            .collect();
        let targets: Vec<&LabelTarget> = views.iter().flat_map(|v| &v.targets).collect();
        found.extend(jobs::label_problems(&ed.doc().header, blocks, &targets));
        // Stable: per block, compile errors first; one entry per block.
        found.sort_by_key(|(i, _)| *i);
        let mut merged: Vec<(usize, String)> = vec![];
        for (i, message) in found {
            match merged.last_mut() {
                Some((last, all)) if *last == i => {
                    all.push_str("; ");
                    all.push_str(&message);
                }
                _ => merged.push((i, message)),
            }
        }
        merged
            .into_iter()
            .filter_map(|(i, message)| Some((blocks.get(i)?.id, message)))
            .collect()
    }

    /// Put the caret at the start of top-level block `id` and show it.
    pub(crate) fn go_to_block(&self, id: NodeId) {
        {
            let mut ed = self.editor.borrow_mut();
            let Some(block) = ed.doc().blocks.iter().find(|b| b.id == id) else {
                return;
            };
            let caret = ed.doc().start_of(block);
            ed.set_caret(caret);
        }
        self.goal_x.set(None);
        self.after_jump();
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
    /// and outlined like an environment, one contour around the lines it
    /// spans; stronger when the next Backspace unwraps it.
    fn snapshot_call_outline(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let (block, call, marked) = {
            let ed = self.editor.borrow();
            let Some(call) = ed.call_focus() else {
                return;
            };
            (ed.caret().block, call, ed.call_marked())
        };
        let Some(index) = self.editor.borrow().doc().top_index(block) else {
            return;
        };
        let Some(&top) = self.block_tops().get(index) else {
            return;
        };
        let stops = self.block_stops(index);
        let lines = caret::call_outline(&stops, block, &call);
        self.snapshot_contour(snapshot, &lines, x0, top - scroll, marked);
    }

    /// A call's outline: the contour of its lines' boxes (`lines`, block
    /// coordinates; the block's top at widget y `top`), tinted; `strong`
    /// for a call marked for unwrapping.
    fn snapshot_contour(
        &self,
        snapshot: &gtk::Snapshot,
        lines: &[typst_bridge::Rect],
        x0: f64,
        top: f64,
        strong: bool,
    ) {
        let (width, alpha) = if strong { (2.0, 0.8) } else { (1.0, 0.35) };
        let zoom = self.zoom.get();
        let blue = |a: f32| gdk::RGBA::new(0.21, 0.52, 0.89, a);
        for polygon in caret::contour(lines, caret::CALL_PAD_PT) {
            let path = gsk::PathBuilder::new();
            for (i, &(x, y)) in polygon.iter().enumerate() {
                let (x, y) = ((x0 + x * zoom) as f32, (top + y * zoom) as f32);
                if i == 0 {
                    path.move_to(x, y);
                } else {
                    path.line_to(x, y);
                }
            }
            path.close();
            let path = path.to_path();
            snapshot.append_fill(&path, gsk::FillRule::Winding, &blue(0.04));
            snapshot.append_stroke(&path, &gsk::Stroke::new(width), &blue(alpha));
        }
    }

    /// The selected range, under the text: a band per line from the first
    /// to the last selected position on it; whole blocks entirely.
    fn snapshot_selection(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let color = gdk::RGBA::new(0.21, 0.52, 0.89, 0.25);
        let ed = self.editor.borrow();
        let tops = self.block_tops();
        if let Some(model::SelRange::Blocks { start, end }) = ed.selection() {
            let views = self.views.borrow();
            for i in start..end {
                let (Some(&top), Some(view)) = (tops.get(i), views.get(i)) else {
                    continue;
                };
                let w = self.page_width_pt.get() * zoom;
                let rect = graphene::Rect::new(
                    x0 as f32,
                    (top - scroll) as f32,
                    w as f32,
                    (view.height_pt() * zoom) as f32,
                );
                snapshot.append_color(&color, &rect);
            }
            return;
        }
        let Some((first, last)) = ed.selection_bounds() else {
            return;
        };
        drop(ed);
        for rect in self.range_rects(&first, &last, x0, scroll) {
            snapshot.append_color(&color, &rect);
        }
    }

    /// Open remarks, under the text: a range is shaded yellow; every remark
    /// gets a yellow bar in the paper's left margin (a range's beside its
    /// lines, a whole block's beside the block).
    fn snapshot_remarks(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let shade = gdk::RGBA::new(1.0, 0.82, 0.15, 0.35);
        let bar = gdk::RGBA::new(0.95, 0.7, 0.0, 0.9);
        let (ranges, blocks) = {
            let ed = self.editor.borrow();
            let doc = ed.doc();
            let open = doc.remarks.iter().filter(|r| !r.resolved);
            let mut ranges = vec![];
            let mut blocks = vec![];
            for remark in open {
                match (remark.anchor, doc.remark_range(remark)) {
                    (_, Some(range)) => ranges.push(range),
                    (model::Anchor::Block(id), None) => blocks.push(id),
                    // A range that does not resolve (detached): not shown.
                    (model::Anchor::Range { .. }, None) => {}
                }
            }
            (ranges, blocks)
        };
        // Every remark gets a bar in the margin: a range beside its lines.
        let mut bars = vec![];
        for (first, last) in &ranges {
            let rects = self.range_rects(first, last, x0, scroll);
            for rect in &rects {
                snapshot.append_color(&shade, rect);
            }
            bars.extend(Self::rects_extent(&rects));
        }
        for id in blocks {
            if let Some((y, h)) = self.block_extent(id) {
                bars.push((y - scroll, h));
            }
        }
        for (y, h) in bars {
            let rect = graphene::Rect::new(
                (x0 - PAPER_PAD_PT * zoom / 2.0 - 1.5) as f32,
                y as f32,
                3.0,
                h as f32,
            );
            snapshot.append_color(&bar, &rect);
        }
    }

    /// The vertical span (top, height) of `rects`, if any.
    fn rects_extent(rects: &[graphene::Rect]) -> Option<(f64, f64)> {
        let top = rects.iter().map(|r| r.y()).reduce(f32::min)?;
        let bottom = rects.iter().map(|r| r.y() + r.height()).reduce(f32::max)?;
        Some((top as f64, (bottom - top) as f64))
    }

    /// The open remarks drawn at widget point `(x, y)` (on a shaded range
    /// or a block's bar): author, date and text of each.
    fn remark_at(&self, x: f64, y: f64) -> Option<String> {
        let zoom = self.zoom.get();
        let scroll = self.scroll();
        let x0 = self.page_x(self.obj().width() as f64);
        let remarks: Vec<model::Remark> = {
            let ed = self.editor.borrow();
            ed.doc()
                .remarks
                .iter()
                .filter(|r| !r.resolved)
                .cloned()
                .collect()
        };
        let on_bar = |extent: Option<(f64, f64)>| {
            let bar = x0 - PAPER_PAD_PT * zoom / 2.0;
            (x - bar).abs() <= 6.0 && extent.is_some_and(|(top, h)| (top..=top + h).contains(&y))
        };
        let mut found = vec![];
        for remark in &remarks {
            let range = self.editor.borrow().doc().remark_range(remark);
            let hit = match (remark.anchor, range) {
                (_, Some((first, last))) => {
                    let rects = self.range_rects(&first, &last, x0, scroll);
                    on_bar(Self::rects_extent(&rects))
                        || rects
                            .iter()
                            .any(|r| r.contains_point(&graphene::Point::new(x as f32, y as f32)))
                }
                (model::Anchor::Block(id), None) => {
                    on_bar(self.block_extent(id).map(|(top, h)| (top - scroll, h)))
                }
                _ => false,
            };
            if hit {
                found.push(format!(
                    "{} ({}): {}",
                    remark.author, remark.date, remark.text
                ));
            }
        }
        (!found.is_empty()).then(|| found.join("\n\n"))
    }

    /// Where block `id` (at any depth) is: its top in content coordinates
    /// and its height, in widget pixels. A nested block spans its caret
    /// positions' lines.
    fn block_extent(&self, id: NodeId) -> Option<(f64, f64)> {
        let zoom = self.zoom.get();
        let (index, top_level) = {
            let ed = self.editor.borrow();
            let doc = ed.doc();
            let index = doc.top_index(id)?;
            (index, doc.blocks[index].id == id)
        };
        let top = *self.block_tops().get(index)?;
        if top_level {
            let height = self.views.borrow().get(index)?.height_pt();
            return Some((top, height * zoom));
        }
        let stops = self.block_stops(index);
        let ed = self.editor.borrow();
        let block = ed.doc().block(id)?;
        let inside = stops.iter().filter(|s| block.contains(s.cursor.block));
        let (y0, y1) = inside.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| {
            (a.min(s.top), b.max(s.bottom))
        });
        (y0 < y1).then_some((top + y0 * zoom, (y1 - y0) * zoom))
    }

    /// The rectangles, line by line, covering every caret position from
    /// `first` to `last` (in one block), in widget coordinates.
    fn range_rects(
        &self,
        first: &CursorPath,
        last: &CursorPath,
        x0: f64,
        scroll: f64,
    ) -> Vec<graphene::Rect> {
        let zoom = self.zoom.get();
        let Some(index) = self.editor.borrow().doc().top_index(first.block) else {
            return vec![];
        };
        let Some(&top) = self.block_tops().get(index) else {
            return vec![];
        };
        let stops = self.block_stops(index);
        let (Some(a), Some(b)) = (
            stops.iter().position(|s| s.cursor == *first),
            stops.iter().position(|s| s.cursor == *last),
        ) else {
            return vec![];
        };
        let inside: Vec<Stop> = stops[a.min(b)..=a.max(b)]
            .iter()
            .filter(|s| !s.tab && !s.label)
            .cloned()
            .collect();
        let mut rects = vec![];
        for line in caret::lines(&inside) {
            let x_min = line
                .iter()
                .map(|&i| inside[i].x)
                .fold(f64::INFINITY, f64::min);
            let x_max = line
                .iter()
                .map(|&i| inside[i].x)
                .fold(f64::NEG_INFINITY, f64::max);
            let y_min = line
                .iter()
                .map(|&i| inside[i].top)
                .fold(f64::INFINITY, f64::min);
            let y_max = line
                .iter()
                .map(|&i| inside[i].bottom)
                .fold(f64::NEG_INFINITY, f64::max);
            rects.push(graphene::Rect::new(
                (x0 + x_min * zoom) as f32,
                (top - scroll + y_min * zoom) as f32,
                ((x_max - x_min) * zoom).max(1.0) as f32,
                ((y_max - y_min) * zoom) as f32,
            ));
        }
        rects
    }

    /// The math call with an options tab that the caret is in: its block's
    /// stops, the block's top (widget y), the caret's block and the call's
    /// path.
    fn tab_call(&self, scroll: f64) -> Option<TabCall> {
        let ed = self.editor.borrow();
        let path = ed.tab_focus()?;
        let block = ed.caret().block;
        let index = ed.doc().top_index(block)?;
        drop(ed);
        let top = *self.block_tops().get(index)? - scroll;
        Some((self.block_stops(index), top, block, path))
    }

    /// The math call with an options tab that the caret is in: outlined
    /// like an inline call (under the render).
    fn snapshot_math_call_outline(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let Some((stops, top, block, path)) = self.tab_call(scroll) else {
            return;
        };
        let lines = caret::call_outline(&stops, block, &path);
        self.snapshot_contour(snapshot, &lines, x0, top, false);
    }

    /// The options tab of the math call the caret is in, on the top edge of
    /// its outline, over the render (it may cover the line above): its
    /// options, or a faint "+ options" when it has none and the caret is
    /// not in it. The text is drawn char by char at the tab's positions.
    fn snapshot_call_tab(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let Some((stops, top, block, mut path)) = self.tab_call(scroll) else {
            return;
        };
        path.push(model::Step::Opts);
        let tab: Vec<&Stop> = stops
            .iter()
            .filter(|s| s.tab && s.cursor.block == block && s.cursor.path == path)
            .collect();
        let (Some(first), Some(last)) = (tab.first(), tab.last()) else {
            return;
        };
        let (opts, editing) = {
            let ed = self.editor.borrow();
            let caret = ed.caret();
            let editing = caret.block == block && caret.path == path;
            let opts = match ed.doc().resolve(&CursorPath {
                block,
                path: path.clone(),
                offset: 0,
            }) {
                Some(model::Target::Opts { text, .. }) => text.to_string(),
                _ => return,
            };
            (opts, editing)
        };
        let shown = if opts.is_empty() && !editing {
            "+ options".to_string()
        } else {
            opts.clone()
        };
        let zoom = self.zoom.get();
        let blue = |a: f32| gdk::RGBA::new(0.21, 0.52, 0.89, a);
        let width = (shown.chars().count().max(1) as f64 * caret::PILL_CHAR_PT
            + 2.0 * caret::PILL_PAD_PT)
            .max(last.x - first.x + 2.0 * caret::PILL_PAD_PT);
        let rect = graphene::Rect::new(
            (x0 + (first.x - caret::PILL_PAD_PT) * zoom) as f32,
            (top + (first.top - 1.0) * zoom) as f32,
            (width * zoom) as f32,
            (caret::TAB_HEIGHT_PT * zoom) as f32,
        );
        let shape = gsk::RoundedRect::from_rect(rect, 2.0);
        let (fill, border, text) = match (opts.is_empty(), editing) {
            (true, false) => (blue(0.03), blue(0.3), gdk::RGBA::new(0.4, 0.45, 0.55, 0.8)),
            (_, true) => (blue(0.12), blue(0.7), gdk::RGBA::new(0.1, 0.25, 0.5, 1.0)),
            (false, false) => (blue(0.08), blue(0.4), gdk::RGBA::new(0.15, 0.3, 0.55, 1.0)),
        };
        snapshot.push_rounded_clip(&shape);
        snapshot.append_color(&gdk::RGBA::WHITE, &rect);
        snapshot.append_color(&fill, &rect);
        snapshot.pop();
        snapshot.append_border(&shape, &[1.0; 4], &[border; 4]);
        let mut font = gtk::pango::FontDescription::from_string("Monospace");
        font.set_absolute_size(caret::PILL_FONT_PT * zoom * gtk::pango::SCALE as f64);
        let baseline_y = first.top - 1.0 + caret::TAB_HEIGHT_PT - caret::TAB_DESCENT_PT;
        for (k, c) in shown.chars().enumerate() {
            let layout = self.obj().create_pango_layout(Some(&c.to_string()));
            layout.set_font_description(Some(&font));
            let baseline = layout.baseline() as f64 / gtk::pango::SCALE as f64;
            let x = first.x + k as f64 * caret::PILL_CHAR_PT;
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (x0 + x * zoom) as f32,
                (top + baseline_y * zoom - baseline) as f32,
            ));
            snapshot.append_layout(&layout, &text);
            snapshot.restore();
        }
    }

    /// The labels offered while a reference is typed: those containing what
    /// was typed, in document order, each with what it labels and its number
    /// once known ("section 2.1", "equation 3", "lemma 1"); then the
    /// bibliography's keys ("citation").
    fn completions(&self) -> Vec<(String, String)> {
        let ed = self.editor.borrow();
        let Some(typed) = ed.ref_typing() else {
            return vec![];
        };
        if *self.completion_typed.borrow() != typed {
            self.completion.set(0);
            *self.completion_typed.borrow_mut() = typed.clone();
        }
        let views = self.views.borrow();
        let numbers: HashMap<&str, String> = views
            .iter()
            .flat_map(|v| &v.targets)
            .map(|t| {
                let n: Vec<String> = t.value.iter().map(i64::to_string).collect();
                (t.label.as_str(), n.join("."))
            })
            .collect();
        ed.doc()
            .labels()
            .into_iter()
            .filter(|(label, _)| label.contains(typed.as_str()))
            .map(|(label, block)| {
                let kind = match &block.kind {
                    Block::Heading { .. } => "section",
                    Block::DisplayMath(_) => "equation",
                    Block::Env { name, .. } => name.as_str(),
                    _ => "",
                };
                let desc = match numbers.get(label) {
                    Some(n) => format!("{kind} {n}"),
                    None => kind.to_string(),
                };
                (label.to_string(), desc)
            })
            // Then the bibliography's keys: citations.
            .chain(
                ed.doc()
                    .bib_keys()
                    .into_iter()
                    .filter(|key| key.contains(typed.as_str()))
                    .map(|key| (key, "citation".to_string())),
            )
            .collect()
    }

    /// Keys for the completion list while it shows: ↑/↓ choose, Enter or
    /// Tab take the chosen label. Returns whether the key was used.
    fn completion_key(&self, key: gdk::Key) -> bool {
        let list = self.completions();
        if list.is_empty() {
            return false;
        }
        let i = self.completion.get().min(list.len() - 1);
        match key {
            gdk::Key::Down => self.completion.set((i + 1) % list.len()),
            gdk::Key::Up => self.completion.set((i + list.len() - 1) % list.len()),
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Tab => {
                self.choose_completion(&list[i].0);
                return true;
            }
            _ => return false,
        }
        self.obj().queue_draw();
        true
    }

    fn choose_completion(&self, label: &str) {
        self.editor.borrow_mut().complete_ref(label);
        self.after_command(true);
    }

    /// The completion row at widget point `(x, y)`, if the list shows.
    fn completion_at(&self, x: f64, y: f64) -> Option<String> {
        self.completion_rows
            .borrow()
            .iter()
            .find(|(r, _)| r.contains_point(&graphene::Point::new(x as f32, y as f32)))
            .map(|(_, label)| label.clone())
    }

    /// The completion list, below the caret: each label with what it labels,
    /// the chosen one highlighted.
    fn snapshot_completions(&self, snapshot: &gtk::Snapshot) {
        let list = self.completions();
        let mut rows = self.completion_rows.borrow_mut();
        rows.clear();
        let Some(caret) = self.caret_rect().filter(|_| !list.is_empty()) else {
            return;
        };
        let obj = self.obj();
        let layouts: Vec<_> = list
            .iter()
            .map(|(label, desc)| {
                let l = obj.create_pango_layout(Some(label));
                let d = obj.create_pango_layout(Some(desc));
                (l, d)
            })
            .collect();
        let (pad, gap) = (6.0_f32, 18.0_f32);
        let label_w = layouts
            .iter()
            .map(|(l, _)| l.pixel_size().0)
            .max()
            .unwrap_or(0) as f32;
        let desc_w = layouts
            .iter()
            .map(|(_, d)| d.pixel_size().0)
            .max()
            .unwrap_or(0) as f32;
        let row_h = layouts
            .iter()
            .map(|(l, _)| l.pixel_size().1)
            .max()
            .unwrap_or(16) as f32
            + 4.0;
        let (x, y) = (caret.x(), caret.y() + caret.height() + 4.0);
        let width = label_w + gap + desc_w + 2.0 * pad;
        let shown = list.len().min(8);
        let frame = graphene::Rect::new(x, y, width, row_h * shown as f32 + 2.0 * 3.0);
        let outline = gsk::RoundedRect::from_rect(frame, 4.0);
        snapshot.push_rounded_clip(&outline);
        snapshot.append_color(&gdk::RGBA::WHITE, &frame);
        snapshot.pop();
        snapshot.append_border(
            &outline,
            &[1.0; 4],
            &[gdk::RGBA::new(0.0, 0.0, 0.0, 0.25); 4],
        );
        let chosen = self.completion.get().min(list.len() - 1);
        // Keep the chosen row in the window of rows shown.
        let first = chosen.saturating_sub(shown - 1);
        for (k, ((label, _), (l, d))) in list
            .iter()
            .zip(&layouts)
            .enumerate()
            .skip(first)
            .take(shown)
        {
            let top = y + 3.0 + row_h * (k - first) as f32;
            let row = graphene::Rect::new(x + 2.0, top, width - 4.0, row_h);
            if k == chosen {
                let r = gsk::RoundedRect::from_rect(row, 3.0);
                snapshot.push_rounded_clip(&r);
                snapshot.append_color(&gdk::RGBA::new(0.21, 0.52, 0.89, 0.18), &row);
                snapshot.pop();
            }
            snapshot.save();
            snapshot.translate(&graphene::Point::new(x + pad, top + 2.0));
            snapshot.append_layout(l, &gdk::RGBA::new(0.1, 0.1, 0.12, 1.0));
            snapshot.translate(&graphene::Point::new(label_w + gap, 0.0));
            snapshot.append_layout(d, &gdk::RGBA::new(0.4, 0.42, 0.48, 1.0));
            snapshot.restore();
            rows.push((row, label.clone()));
        }
    }

    /// A light green background behind every reference and hyperlink in
    /// view (a wrapped link gets one box per line), so links stand out
    /// whatever the header's styling (editor only, not in Typst).
    fn snapshot_ref_marks(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
        let zoom = self.zoom.get();
        let height = self.obj().height() as f64;
        let green = gdk::RGBA::new(0.20, 0.65, 0.33, 0.16);
        let ed = self.editor.borrow();
        let views = self.views.borrow();
        let tops = self.block_tops();
        let blocks = ed.doc().blocks.iter().zip(views.iter()).zip(&tops);
        for (index, ((block, view), &top)) in blocks.enumerate() {
            let y0 = top - scroll;
            if y0 > height || y0 + view.height_pt() * zoom < 0.0 {
                continue;
            }
            let Some(layout) = view.layout.as_ref() else {
                continue;
            };
            let mut boxes: Vec<typst_bridge::Rect> = block
                .ref_ids()
                .into_iter()
                .filter_map(|id| layout.box_of(id))
                .collect();
            let links = block.links();
            if !links.is_empty() {
                let stops = self.block_stops(index);
                for (leaf, body) in links {
                    boxes.extend(caret::call_outline(&stops, leaf, &body));
                }
            }
            for r in boxes {
                let rect = graphene::Rect::new(
                    (x0 + r.x * zoom - 1.5) as f32,
                    (y0 + r.y * zoom - 1.0) as f32,
                    (r.w * zoom + 3.0) as f32,
                    (r.h * zoom + 2.0) as f32,
                );
                let rounded = gsk::RoundedRect::from_rect(rect, 2.5);
                snapshot.push_rounded_clip(&rounded);
                snapshot.append_color(&green, &rect);
                snapshot.pop();
            }
        }
    }

    /// Label pills beside the page: every label, and a faint "+ label" for
    /// the block the caret is in (see `Editor::label_focus`). The text is
    /// drawn char by char at the label's caret positions.
    fn snapshot_label_pills(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64, focus: bool) {
        let zoom = self.zoom.get();
        let height = self.obj().height() as f64;
        let (focused, caret) = {
            let ed = self.editor.borrow();
            (ed.label_focus().filter(|_| focus), ed.caret().clone())
        };
        let blue = |a: f32| gdk::RGBA::new(0.21, 0.52, 0.89, a);
        let mut font = gtk::pango::FontDescription::from_string("Monospace");
        font.set_absolute_size(caret::PILL_FONT_PT * zoom * gtk::pango::SCALE as f64);
        let tops = self.block_tops();
        for (index, &top) in tops.iter().enumerate() {
            let y0 = top - scroll;
            let next = tops.get(index + 1).map_or(f64::INFINITY, |t| t - scroll);
            if next < 0.0 || y0 > height {
                continue;
            }
            let stops = self.block_stops(index);
            let ed = self.editor.borrow();
            let mut i = 0;
            while i < stops.len() {
                if !stops[i].label {
                    i += 1;
                    continue;
                }
                let block = stops[i].cursor.block;
                let n = stops[i..]
                    .iter()
                    .take_while(|s| s.label && s.cursor.block == block)
                    .count();
                let (first, last) = (&stops[i], &stops[i + n - 1]);
                i += n;
                let Some(label) = ed.doc().block(block).map(|b| b.label.clone()) else {
                    continue;
                };
                let editing = caret.block == block && caret.path == [model::Step::Label];
                if label.is_empty() && focused != Some(block) {
                    continue;
                }
                let shown = if label.is_empty() && !editing {
                    "+ label".to_string()
                } else {
                    label.clone()
                };
                let width = (shown.chars().count().max(1) as f64 * caret::PILL_CHAR_PT
                    + 2.0 * caret::PILL_PAD_PT)
                    .max(last.x - first.x + 2.0 * caret::PILL_PAD_PT);
                let rect = graphene::Rect::new(
                    (x0 + (first.x - caret::PILL_PAD_PT) * zoom) as f32,
                    (y0 + first.top * zoom) as f32,
                    (width * zoom) as f32,
                    ((first.bottom - first.top) * zoom) as f32,
                );
                let pill = gsk::RoundedRect::from_rect(rect, rect.height() / 2.0);
                let (fill, border, text) = match (label.is_empty(), editing) {
                    (true, false) => (blue(0.03), blue(0.25), gdk::RGBA::new(0.4, 0.45, 0.55, 0.8)),
                    (_, true) => (blue(0.12), blue(0.7), gdk::RGBA::new(0.1, 0.25, 0.5, 1.0)),
                    (false, false) => {
                        (blue(0.08), blue(0.35), gdk::RGBA::new(0.15, 0.3, 0.55, 1.0))
                    }
                };
                // A label defined twice: references to it cannot resolve.
                let twice = !label.is_empty()
                    && ed
                        .doc()
                        .labels()
                        .iter()
                        .filter(|(l, _)| *l == label)
                        .count()
                        > 1;
                let red = gdk::RGBA::new(0.75, 0.11, 0.16, 1.0);
                let (border, text) = if twice { (red, red) } else { (border, text) };
                snapshot.push_rounded_clip(&pill);
                snapshot.append_color(&fill, &rect);
                snapshot.pop();
                snapshot.append_border(&pill, &[1.0; 4], &[border; 4]);
                for (k, c) in shown.chars().enumerate() {
                    let layout = self.obj().create_pango_layout(Some(&c.to_string()));
                    layout.set_font_description(Some(&font));
                    let baseline = layout.baseline() as f64 / gtk::pango::SCALE as f64;
                    let x = first.x + k as f64 * caret::PILL_CHAR_PT;
                    snapshot.save();
                    snapshot.translate(&graphene::Point::new(
                        (x0 + x * zoom) as f32,
                        (y0 + (first.bottom - caret::PILL_BELOW_PT) * zoom - baseline) as f32,
                    ));
                    snapshot.append_layout(&layout, &text);
                    snapshot.restore();
                }
            }
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

    /// The label of the reference drawn at widget point `(x, y)`, if any.
    fn ref_at(&self, x: f64, y: f64) -> Option<String> {
        let zoom = self.zoom.get();
        let cy = y + self.scroll();
        let tops = self.block_tops();
        let index = tops.iter().rposition(|&t| t <= cy)?;
        let bx = (x - self.page_x(self.obj().width() as f64)) / zoom;
        let by = (cy - tops[index]) / zoom;
        let id = self
            .views
            .borrow()
            .get(index)?
            .layout
            .as_ref()?
            .hit(bx, by)?;
        let ed = self.editor.borrow();
        ed.doc().ref_label(id).map(str::to_string)
    }

    fn locate(&self, x: f64, y: f64) -> Option<CursorPath> {
        if let Some(c) = self.tab_at(x, y) {
            return Some(c);
        }
        let (stops, bx, by) = self.block_at(x, y)?;
        caret::hit(&stops, bx, by).map(|s| s.cursor.clone())
    }

    /// The position in the options tab shown (of the call the caret is in)
    /// at widget point `(x, y)`. The tab may stick out above its block.
    fn tab_at(&self, x: f64, y: f64) -> Option<CursorPath> {
        let (block, mut path) = {
            let ed = self.editor.borrow();
            (ed.caret().block, ed.tab_focus()?)
        };
        path.push(model::Step::Opts);
        let index = self.editor.borrow().doc().top_index(block)?;
        let top = *self.block_tops().get(index)?;
        let zoom = self.zoom.get();
        let bx = (x - self.page_x(self.obj().width() as f64)) / zoom;
        let by = (y + self.scroll() - top) / zoom;
        let stops = self.block_stops(index);
        caret::tab_hit(&stops, block, &path, bx, by).map(|s| s.cursor.clone())
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
        if self.reveal_third.take() {
            // As far as the document's ends allow.
            let wanted = value + r.y() as f64 - page / 3.0;
            adj.set_value(wanted.clamp(adj.lower(), (adj.upper() - page).max(adj.lower())));
        } else if top < 0.0 {
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

    /// Which top-level block is being typed in: the caret's, once it has
    /// changed while the caret was in it, until the caret leaves it. (Not
    /// on merely entering it, so reading with the caret reflows nothing.)
    fn track_editing(&self, ed: &Editor) {
        let doc = ed.doc();
        let top = doc.top_index(ed.caret().block).map(|i| &doc.blocks[i]);
        let mut last = self.caret_block.borrow_mut();
        let edited = matches!((top, last.as_ref()), (Some(b), Some(l)) if b.id == l.id && b != l);
        let editing = match top {
            Some(b) if edited || self.editing.get() == Some(b.id) => Some(b.id),
            _ => None,
        };
        self.editing.set(editing);
        *last = top.cloned();
    }

    /// Submit compile jobs for blocks whose source changed, or for all
    /// blocks if `force` (render settings changed).
    fn submit_blocks(&self, force: bool) {
        if self.page_width_pt.get() <= 0.0 {
            return; // Not allocated yet; size_allocate will submit.
        }
        let ed = self.editor.borrow();
        self.track_editing(&ed);
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
                    start: v.start.as_ref(),
                    targets: &v.targets,
                })
                .collect();
            jobs::stale_blocks_editing(
                &ed.doc().blocks,
                &ed.doc().header,
                &ed.doc().files,
                self.show_code.get(),
                &submitted,
                self.editing.get(),
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
                        files: view.around.files.clone(),
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

    /// Render heading `id` as an outline entry: its last job (header,
    /// counters before it, its snippet) with `outline_entry_rule`, in the
    /// widget's text colour.
    fn submit_outline_entry(&self, id: NodeId) {
        let (around, snippet) = {
            let views = self.views.borrow();
            let Some(view) = views.iter().find(|v| v.id == id) else {
                return;
            };
            (view.around.clone(), view.snippet.clone())
        };
        let c = self.obj().color();
        let hex = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = format!(
            "rgb(\"#{:02x}{:02x}{:02x}\")",
            hex(c.red()),
            hex(c.green()),
            hex(c.blue())
        );
        let mut around = around;
        around.prefix.push_str(&outline_entry_rule(&color));
        let ppp = OUTLINE_PX_PER_PT * self.obj().scale_factor() as f64;
        let key = (around.clone(), snippet.clone(), ppp);
        let generation = {
            let mut outline = self.outline.borrow_mut();
            let entry = outline.entry(id).or_default();
            if entry.submitted.as_ref() == Some(&key) {
                return;
            }
            let generation = self.generation.get() + 1;
            self.generation.set(generation);
            entry.generation = generation;
            entry.submitted = Some(key);
            generation
        };
        self.worker.get().expect("worker").submit(CompileJob {
            block: NodeId(id.0 | OUTLINE_BIT),
            generation,
            around,
            snippet,
            map: SpanMap::default(),
            width_pt: OUTLINE_WIDTH_PT,
            pixel_per_pt: ppp,
            persist: false,
            transient: false,
        });
    }

    fn on_outline_result(&self, result: CompileResult) {
        let id = NodeId(result.block.0 & !OUTLINE_BIT);
        {
            let mut outline = self.outline.borrow_mut();
            let Some(entry) = outline.get_mut(&id) else {
                return;
            };
            let Ok(rendered) = result.outcome else {
                return; // The last good render stays.
            };
            if result.generation != entry.generation {
                return;
            }
            // The page's content box (entries align, all lines the same
            // height), widened to the ink, to the ink's right end.
            let raster = rendered.raster;
            let pad = (BLOCK_PAD_PT * rendered.pixel_per_pt) as u32;
            let Some((x, y, w, h)) = raster.ink_bounds() else {
                return;
            };
            let (left, top) = (pad.min(x), pad.min(y));
            let bottom = (raster.height.saturating_sub(pad)).max(y + h);
            let cropped = raster.crop((left, top, x + w - left, bottom - top));
            let scale = rendered.pixel_per_pt / OUTLINE_PX_PER_PT;
            entry.texture = Some((texture(Arc::new(cropped)), scale));
        }
        self.obj().emit_by_name::<()>("outline-rendered", &[&id.0]);
    }

    /// The outline entry of heading `id` (see `submit_outline_entry`) and
    /// its device pixels per logical pixel, once rendered.
    pub(crate) fn outline_entry(&self, id: NodeId) -> Option<(gdk::Texture, f64)> {
        self.outline.borrow().get(&id)?.texture.clone()
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
        if result.block.0 & OUTLINE_BIT != 0 {
            self.on_outline_result(result);
            return;
        }
        let heading = matches!(
            self.editor
                .borrow()
                .doc()
                .block(result.block)
                .map(|b| &b.kind),
            Some(Block::Heading { .. })
        );
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
            let latest = result.generation == view.submitted;
            let mut retargeted = false;
            let end = match result.outcome {
                Ok(rendered) => {
                    view.texture = Some(texture(rendered.raster));
                    view.texture_ppp = rendered.pixel_per_pt;
                    view.layout = Some(rendered.layout);
                    view.error = None;
                    // New label targets: references to them render again.
                    if latest && view.targets != rendered.targets {
                        view.targets = rendered.targets;
                        retargeted = true;
                    }
                    rendered.end_state
                }
                Err(errors) => {
                    view.error = Some(errors.join("\n"));
                    None
                }
            };
            // The counters this block ends with; a failed block passes its
            // start on unchanged.
            let current = latest && view.start.is_some();
            if current {
                view.end = end.or_else(|| view.start.clone());
            }
            current || retargeted
        };
        if heading {
            self.submit_outline_entry(result.block);
        }
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
    let alt = state.contains(gdk::ModifierType::ALT_MASK);
    use gdk::Key;
    Some(match key {
        Key::Left | Key::KP_Left if alt => Command::Back,
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
        Key::Return | Key::KP_Enter | Key::ISO_Enter if shift => Command::LineBreak,
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
            Key::c => Command::Copy,
            Key::x => Command::Cut,
            Key::v => Command::Paste,
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
