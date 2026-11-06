use std::cell::{Cell, OnceCell, RefCell};
use std::sync::{Arc, OnceLock};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};
use model::{Document, NodeId};
use typst_bridge::{
    BLOCK_PAD_PT, BlockLayout, CompileJob, CompileResult, Raster, Worker, serialize_block,
};

/// Logical pixels per typographic point at 100 % zoom.
const ZOOM: f64 = 1.5;
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
    layout: Option<BlockLayout>,
    /// Message of the last failed compile; the last good render stays.
    error: Option<String>,
}

impl BlockView {
    fn height_pt(&self) -> f64 {
        self.layout
            .as_ref()
            .map_or(0.0, |l| l.height)
            .max(MIN_BLOCK_HEIGHT_PT)
    }
}

#[derive(Default)]
pub struct SemEditor {
    pub(crate) doc: RefCell<Document>,
    views: RefCell<Vec<BlockView>>,
    worker: OnceCell<Worker>,
    generation: Cell<u64>,
    /// Page width and pixels per point the current renders were requested at.
    page_width_pt: Cell<f64>,
    pixel_per_pt: Cell<f64>,

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
        obj.set_overflow(gtk::Overflow::Hidden);

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

        obj.connect_scale_factor_notify(|obj| obj.queue_allocate());
    }
}

impl WidgetImpl for SemEditor {
    fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
        match orientation {
            gtk::Orientation::Horizontal => {
                let min = MIN_PAGE_WIDTH_PT * ZOOM + 2.0 * SIDE_MARGIN_PX;
                let nat = MAX_PAGE_WIDTH_PT * ZOOM + 2.0 * (SIDE_MARGIN_PX + PAPER_PAD_PT * ZOOM);
                (min as i32, nat as i32, -1, -1)
            }
            _ => (0, self.content_height() as i32, -1, -1),
        }
    }

    fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
        let page = ((width as f64 - 2.0 * (SIDE_MARGIN_PX + PAPER_PAD_PT * ZOOM)) / ZOOM)
            .clamp(MIN_PAGE_WIDTH_PT, MAX_PAGE_WIDTH_PT)
            .floor();
        let ppp = ZOOM * self.device_scale();
        if page != self.page_width_pt.get() || ppp != self.pixel_per_pt.get() {
            self.page_width_pt.set(page);
            self.pixel_per_pt.set(ppp);
            self.submit_blocks(true);
        }
        self.configure_adjustments(width as f64, height as f64);
    }

    fn snapshot(&self, snapshot: &gtk::Snapshot) {
        let obj = self.obj();
        let (width, height) = (obj.width() as f64, obj.height() as f64);
        let scroll = self
            .vadjustment
            .borrow()
            .as_ref()
            .map_or(0.0, |a| a.value());
        let page_px = self.page_width_pt.get() * ZOOM;
        let x0 = self.page_x(width);
        let pad = PAPER_PAD_PT * ZOOM;

        // Paper.
        let paper = graphene::Rect::new(
            (x0 - pad) as f32,
            (TOP_MARGIN_PX - pad - scroll) as f32,
            (page_px + 2.0 * pad) as f32,
            (self.content_height() - 2.0 * TOP_MARGIN_PX + 2.0 * pad) as f32,
        );
        snapshot.append_color(&gdk::RGBA::WHITE, &paper);

        let mut y = TOP_MARGIN_PX - scroll;
        for view in self.views.borrow().iter() {
            let h = view.height_pt() * ZOOM;
            if y + h >= 0.0 && y <= height {
                if let (Some(texture), Some(layout)) = (&view.texture, &view.layout) {
                    // The raster has a transparent pad around the content.
                    let pad = BLOCK_PAD_PT * ZOOM;
                    let rect = graphene::Rect::new(
                        (x0 - pad) as f32,
                        (y - pad) as f32,
                        (layout.width * ZOOM + 2.0 * pad) as f32,
                        (layout.height * ZOOM + 2.0 * pad) as f32,
                    );
                    snapshot.append_texture(texture, &rect);
                }
                if view.error.is_some() {
                    let marker =
                        graphene::Rect::new((x0 - 14.0) as f32, (y + 5.0) as f32, 6.0, 6.0);
                    snapshot.append_color(&gdk::RGBA::new(0.88, 0.11, 0.14, 1.0), &marker);
                }
            }
            y += h + BLOCK_SPACING_PT * ZOOM;
        }
    }
}

impl ScrollableImpl for SemEditor {}

impl SemEditor {
    pub(crate) fn set_document(&self, doc: Document) {
        let views = doc
            .blocks
            .iter()
            .map(|b| BlockView {
                id: b.id,
                snippet: String::new(),
                submitted: 0,
                shown: 0,
                texture: None,
                layout: None,
                error: None,
            })
            .collect();
        *self.doc.borrow_mut() = doc;
        *self.views.borrow_mut() = views;
        self.submit_blocks(true);
        self.obj().queue_resize();
    }

    fn device_scale(&self) -> f64 {
        let obj = self.obj();
        obj.native()
            .and_then(|n| n.surface())
            .map_or(obj.scale_factor() as f64, |s| s.scale())
    }

    /// Left edge of the text column in widget coordinates.
    fn page_x(&self, width: f64) -> f64 {
        ((width - self.page_width_pt.get() * ZOOM) / 2.0).floor()
    }

    fn content_height(&self) -> f64 {
        let views = self.views.borrow();
        let blocks: f64 = views
            .iter()
            .map(|v| (v.height_pt() + BLOCK_SPACING_PT) * ZOOM)
            .sum();
        blocks + 2.0 * TOP_MARGIN_PX
    }

    /// Submit compile jobs for blocks whose source changed, or for all
    /// blocks if `force` (render settings changed).
    fn submit_blocks(&self, force: bool) {
        if self.page_width_pt.get() <= 0.0 {
            return; // Not allocated yet; size_allocate will submit.
        }
        let doc = self.doc.borrow();
        let mut views = self.views.borrow_mut();
        for (block, view) in doc.blocks.iter().zip(views.iter_mut()) {
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
                    view.layout = Some(rendered.layout);
                    view.error = None;
                }
                Err(errors) => view.error = Some(errors.join("\n")),
            }
        }
        let obj = self.obj();
        obj.queue_resize();
        obj.queue_draw();
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
