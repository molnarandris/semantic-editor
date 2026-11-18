//! The `SemEditor` widget's implementation: its state, the GObject and
//! widget overrides, page geometry and the caret. Input, drawing and
//! compile scheduling are in the submodules.

mod compile;
mod draw;
mod input;
mod popup;
mod quote;

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
    snippet_rule,
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

/// A code piece or call that failed to compile (`jobs::failing_source`).
struct Failed {
    /// Its top-level block.
    block: NodeId,
    /// Its source when it failed.
    source: String,
    /// The error.
    message: String,
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

/// Job ids of snippet renders (`SemEditor::render_snippet`): a counter
/// with this bit set.
const SNIPPET_BIT: u64 = 1 << 61;
/// Snippets: logical pixels per pt.
pub const SNIPPET_PX_PER_PT: f64 = 1.25;
/// The width of a citation's full reference on hovering it, in points.
const CITATION_TIP_PT: f64 = 320.0;

/// What a snippet render calls with: the texture and its device pixels per
/// logical pixel, or the compile errors.
pub type SnippetDone = Box<dyn FnOnce(Result<(gdk::Texture, f64), String>)>;

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
    /// Snippet renders waiting for their result, by job id.
    snippets: RefCell<HashMap<u64, SnippetDone>>,
    next_snippet: Cell<u64>,
    /// Code pieces and calls that failed to compile, shown as their source
    /// on red while it is as it was then.
    failed: RefCell<HashMap<NodeId, Failed>>,
    /// Full references shown on hovering citations, by the header and the
    /// Typst code rendered (`jobs::full_citation`); None while rendering or
    /// if it failed.
    cite_tips: RefCell<HashMap<String, Option<gdk::Paintable>>>,
    /// Labels of the paper that references in this text may point to (a
    /// remark's text); see `set_outside`.
    outside: RefCell<Vec<jobs::Outside>>,
    /// Who this editor is to the shared compile thread (see [`submit`]).
    owner: Cell<u64>,
    /// As tall as its content, no paper or margins (a remark's text in its
    /// card); see `set_fit_content`.
    pub(crate) fit: Cell<bool>,
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
    /// Compiling and `caret-moved` are waiting for the next frame (see
    /// `after_key`).
    after_key_pending: Cell<bool>,
    /// The completion list while a reference is typed: the chosen entry,
    /// the label typed when it was chosen (a new one starts at the top),
    /// and the pop-up showing the list (see `popup.rs`).
    completion: Cell<usize>,
    completion_typed: RefCell<String>,
    popup: OnceCell<(gtk::Popover, gtk::ListBox)>,

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

thread_local! {
    /// The compile thread all editors share (the paper's, the remark
    /// cards', old versions'), started with the first editor.
    static WORKER: Worker = {
        let disk_cache = glib::user_cache_dir()
            .join("semantic-editor")
            .join("renders");
        // Results arrive on the compile thread; hop to the main loop, to
        // the editor that asked.
        Worker::spawn(Some(disk_cache), move |result| {
            glib::MainContext::default().invoke(move || {
                let editor = EDITORS.with_borrow(|e| e.get(&result.owner).and_then(|w| w.upgrade()));
                if let Some(editor) = editor {
                    editor.imp().on_result(result);
                }
            });
        })
    };
    /// The editors by owner id, for the results.
    static EDITORS: RefCell<HashMap<u64, glib::WeakRef<super::SemEditor>>> =
        RefCell::default();
    static NEXT_OWNER: Cell<u64> = const { Cell::new(1) };
}

/// Register `editor` for compile results: its owner id.
fn register(editor: &super::SemEditor) -> u64 {
    let owner = NEXT_OWNER.replace(NEXT_OWNER.get() + 1);
    EDITORS.with_borrow_mut(|e| e.insert(owner, editor.downgrade()));
    owner
}

impl SemEditor {
    /// Compile `job` on the shared thread, for this editor.
    pub(super) fn submit(&self, job: CompileJob) {
        WORKER.with(|w| w.submit_as(self.owner.get(), job));
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
                // A block's render arrived (remark quotations follow).
                Signal::builder("block-rendered").build(),
                // Ctrl+click on a reference to an outside label (the
                // paper's, from a remark's text). Param: the label.
                Signal::builder("follow-outside")
                    .param_types([String::static_type()])
                    .build(),
                // A click, in a read-only text (a chat answer), on a link to
                // a block of the paper (`block:` and its id). Param: the id.
                Signal::builder("follow-place")
                    .param_types([u64::static_type()])
                    .build(),
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

        self.owner.set(register(&obj));

        let mut editor = self.editor.borrow_mut();
        editor.set_symbol_resolver(typst_bridge::resolve_symbol);
        editor.set_call_resolver(typst_bridge::call_shape);
        editor.set_math_call_resolver(typst_bridge::math_call_shape);
        drop(editor);
        obj.connect_scale_factor_notify(|obj| obj.queue_allocate());
        self.setup_input();
        self.setup_popup();
    }

    fn dispose(&self) {
        EDITORS.with_borrow_mut(|e| e.remove(&self.owner.get()));
        if let Some(im) = self.im.get() {
            im.set_client_widget(None::<&gtk::Widget>);
        }
        self.stop_blink();
        if let Some((popup, _)) = self.popup.get() {
            popup.unparent();
        }
    }
}

impl WidgetImpl for SemEditor {
    fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
        let zoom = self.zoom.get();
        match orientation {
            gtk::Orientation::Horizontal if self.fit.get() => {
                let min = (MIN_PAGE_WIDTH_PT * zoom) as i32;
                (min, min, -1, -1)
            }
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
        let margins = match self.fit.get() {
            true => 0.0,
            false => 2.0 * (SIDE_MARGIN_PX + PAPER_PAD_PT * zoom),
        };
        let page = ((width as f64 - margins) / zoom)
            .clamp(MIN_PAGE_WIDTH_PT, MAX_PAGE_WIDTH_PT)
            .floor();
        let ppp = zoom * self.device_scale();
        if page != self.page_width_pt.get() || ppp != self.pixel_per_pt.get() {
            self.page_width_pt.set(page);
            self.pixel_per_pt.set(ppp);
            self.submit_blocks(true);
        }
        self.configure_adjustments(width as f64, height as f64);
        if let Some((popup, _)) = self.popup.get() {
            popup.present();
        }
    }

    fn snapshot(&self, snapshot: &gtk::Snapshot) {
        let zoom = self.zoom.get();
        let obj = self.obj();
        let (width, height) = (obj.width() as f64, obj.height() as f64);
        let scroll = self.scroll();
        let page_px = self.page_width_pt.get() * zoom;
        let x0 = self.page_x(width);
        let pad = PAPER_PAD_PT * zoom;

        // Paper (a card around a fitted editor has its own).
        let top_margin = self.top_margin();
        if !self.fit.get() {
            let paper = graphene::Rect::new(
                (x0 - pad) as f32,
                (top_margin - pad - scroll) as f32,
                (page_px + 2.0 * pad) as f32,
                (self.content_height() - 2.0 * top_margin + 2.0 * pad) as f32,
            );
            snapshot.append_color(&gdk::RGBA::WHITE, &paper);
        }
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
        let mut y = top_margin - scroll;
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
    }
}

impl ScrollableImpl for SemEditor {}

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
        self.update_completions();
        self.restart_blink();
        self.obj().emit_by_name::<()>("caret-moved", &[]);
    }

    /// As `after_command`, for a key: the caret shows at once, but finding
    /// the blocks to compile (every block's source, slow in a long paper)
    /// and `caret-moved` wait until just before the next frame, once for
    /// all keys pressed since. Otherwise a held arrow key (auto-repeat
    /// faster than that work) keeps GTK from drawing until it is let go.
    pub(crate) fn after_key(&self) {
        self.sync_views();
        self.obj().queue_resize();
        self.reveal.set(true);
        self.reveal_caret();
        self.update_im_location();
        self.update_completions();
        self.restart_blink();
        if self.after_key_pending.replace(true) {
            return;
        }
        let weak = self.obj().downgrade();
        // Before the redraw (`GDK_PRIORITY_REDRAW` is HIGH_IDLE + 20), after
        // the pending key events.
        glib::idle_add_local_full(
            glib::Priority::from(glib::ffi::G_PRIORITY_HIGH_IDLE + 10),
            move || {
                if let Some(obj) = weak.upgrade() {
                    let imp = obj.imp();
                    imp.after_key_pending.set(false);
                    imp.submit_blocks(false);
                    obj.emit_by_name::<()>("caret-moved", &[]);
                }
                glib::ControlFlow::Break
            },
        );
    }

    /// After the caret jumped somewhere: as `after_command`, with the caret
    /// shown a third down the view.
    /// Go to the block labelled `label` (Alt+← comes back).
    pub(crate) fn follow_ref(&self, label: &str) -> bool {
        if !self.editor.borrow_mut().follow_ref(label) {
            return false;
        }
        self.goal_x.set(None);
        self.after_jump();
        true
    }

    pub(crate) fn after_jump(&self) {
        self.reveal_third.set(true);
        self.after_command(true);
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
        // Pieces that failed: their block renders, the piece as source.
        for f in self.failed.borrow().values() {
            if let Some(i) = blocks.iter().position(|b| b.id == f.block)
                && !found.iter().any(|(j, _)| *j == i)
            {
                found.push((i, f.message.lines().next().unwrap_or("").to_string()));
            }
        }
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
    pub(crate) fn go_to_block(&self, id: NodeId) -> bool {
        {
            let mut ed = self.editor.borrow_mut();
            let Some(block) = ed.doc().block(id) else {
                return false;
            };
            let caret = ed.doc().start_of(block);
            ed.set_caret(caret);
        }
        self.goal_x.set(None);
        self.after_jump();
        true
    }

    /// Select remark `id`'s text (or go to its block) and show it.
    pub(crate) fn select_remark(&self, id: u64) {
        if self.editor.borrow_mut().select_remark(id) {
            self.goal_x.set(None);
            self.after_jump();
        }
    }

    /// Select the structure of breadcrumb entry `path_len`.
    pub(crate) fn select_crumb(&self, path_len: usize) {
        let changed = self.editor.borrow_mut().select_crumb(path_len);
        if changed {
            self.after_command(false);
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
        match self.fit.get() {
            // No spacing after the last block.
            true => (blocks - BLOCK_SPACING_PT * zoom).max(MIN_BLOCK_HEIGHT_PT * zoom),
            false => blocks + 2.0 * TOP_MARGIN_PX,
        }
    }

    /// Space above the first block, px.
    fn top_margin(&self) -> f64 {
        if self.fit.get() { 0.0 } else { TOP_MARGIN_PX }
    }

    /// Top of every block in content coordinates (px, unscrolled).
    fn block_tops(&self) -> Vec<f64> {
        let zoom = self.zoom.get();
        let mut y = self.top_margin();
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

    /// The smallest node drawn at widget point `(x, y)`, if any.
    fn node_at(&self, x: f64, y: f64) -> Option<NodeId> {
        let zoom = self.zoom.get();
        let cy = y + self.scroll();
        let tops = self.block_tops();
        let index = tops.iter().rposition(|&t| t <= cy)?;
        let bx = (x - self.page_x(self.obj().width() as f64)) / zoom;
        let by = (cy - tops[index]) / zoom;
        self.views.borrow().get(index)?.layout.as_ref()?.hit(bx, by)
    }

    /// The label of the reference drawn at widget point `(x, y)`, if any.
    fn ref_at(&self, x: f64, y: f64) -> Option<String> {
        let id = self.node_at(x, y)?;
        let ed = self.editor.borrow();
        ed.doc().ref_label(id).map(str::to_string)
    }

    /// The web address of the link drawn at widget point `(x, y)`, if any.
    fn link_at(&self, x: f64, y: f64) -> Option<String> {
        let id = self.node_at(x, y)?;
        self.editor.borrow().doc().link_url(id)
    }

    /// Open `url` in the browser (through the portal in the Flatpak).
    fn open_link(&self, url: &str) {
        let window = self.obj().root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(url).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
    }

    /// Show in `tooltip` the full reference of the citation at widget point
    /// `(x, y)`, rendered by Typst in the tooltip's white. The first time it
    /// is rendered in the background and the tooltip asked for again.
    fn citation_tip(&self, x: f64, y: f64, tooltip: &gtk::Tooltip) -> bool {
        let Some(key) = self.ref_at(x, y) else {
            return false;
        };
        let Some(code) = jobs::full_citation(&self.editor.borrow().doc().blocks, &key) else {
            return false;
        };
        let cached = format!("{}\n{code}", self.snippet_header());
        if let Some(tip) = self.cite_tips.borrow().get(&cached) {
            let Some(paintable) = tip else {
                return false;
            };
            tooltip.set_custom(Some(&gtk::Picture::for_paintable(paintable)));
            return true;
        }
        self.cite_tips.borrow_mut().insert(cached.clone(), None);
        let weak = self.obj().downgrade();
        let done = move |result: Result<(gdk::Texture, f64), String>| {
            let Some(obj) = weak.upgrade() else {
                return;
            };
            // Shown at its size in logical pixels, sharp on any screen, all
            // white (the header may colour citations or links).
            let paintable = result.ok().and_then(|(texture, scale)| {
                let w = texture.width() as f32 / scale as f32;
                let h = texture.height() as f32 / scale as f32;
                let snapshot = gtk::Snapshot::new();
                let mut keep_alpha = [0.0; 16];
                keep_alpha[15] = 1.0;
                snapshot.push_color_matrix(
                    &graphene::Matrix::from_float(keep_alpha),
                    &graphene::Vec4::new(1.0, 1.0, 1.0, 0.0),
                );
                snapshot.append_texture(&texture, &graphene::Rect::new(0.0, 0.0, w, h));
                snapshot.pop();
                snapshot.to_paintable(Some(&graphene::Size::new(w, h)))
            });
            obj.imp().cite_tips.borrow_mut().insert(cached, paintable);
            obj.trigger_tooltip_query();
        };
        self.render_snippet(&code, CITATION_TIP_PT, gdk::RGBA::WHITE, Box::new(done));
        false
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
