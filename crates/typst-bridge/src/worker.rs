//! Background compilation. One thread owns the [`BridgeWorld`] and compiles
//! block jobs in order; a newer job for the same block replaces a queued
//! older one. Results go to a caller-supplied callback, which the GUI uses to
//! hop back onto its main loop. This crate knows nothing about GTK.
//!
//! Renders are cached in memory by source, width and scale; renders of jobs
//! marked `persist` (diagrams, raw Typst) also on disk.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use model::NodeId;

use crate::TYPST_VERSION;
use crate::compile::{Around, Raster, block_document, compile, render_page};
use crate::counters::{self, CounterState, LabelTarget};
use crate::disk_cache::DiskCache;
use crate::layout::{BlockLayout, RawLayout, raw_layout};
use crate::serialize::SpanMap;
use crate::world::BridgeWorld;

/// Maximum number of cached renders before the cache is emptied.
const CACHE_CAPACITY: usize = 256;

#[derive(Clone, Debug)]
pub struct CompileJob {
    pub block: NodeId,
    /// Increases with every edit of the block; results carry it back so the
    /// caller can ignore stale ones.
    pub generation: u64,
    /// Header and counter code compiled around the snippet.
    pub around: Around,
    pub snippet: String,
    pub map: SpanMap,
    pub width_pt: f64,
    /// Zoom × device scale factor.
    pub pixel_per_pt: f64,
    /// Keep the render in the disk cache too (slow blocks).
    pub persist: bool,
    /// One-off render (e.g. a zoomed preview): not added to any cache, so renders
    /// at arbitrary scales do not pile up.
    pub transient: bool,
}

#[derive(Clone, Debug)]
pub struct Rendered {
    pub raster: Arc<Raster>,
    pub layout: BlockLayout,
    pub pixel_per_pt: f64,
    /// Counter values at the end of the block, if `around` asked for them.
    pub end_state: Option<CounterState>,
    /// What the labels the block defines point to, if `around` asked.
    pub targets: Vec<LabelTarget>,
}

#[derive(Debug)]
pub struct CompileResult {
    pub block: NodeId,
    pub generation: u64,
    pub outcome: Result<Rendered, Vec<String>>,
    /// Compile + render time; zero on a cache hit.
    pub elapsed: Duration,
}

/// Handle to the compile thread. Dropping it stops the thread.
pub struct Worker {
    tx: Sender<CompileJob>,
    compiles: Arc<AtomicUsize>,
}

impl Worker {
    /// Start the compile thread. `disk_cache` is a directory for persisted
    /// renders (`None`: memory only).
    pub fn spawn(
        disk_cache: Option<PathBuf>,
        deliver: impl Fn(CompileResult) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let compiles = Arc::new(AtomicUsize::new(0));
        let counter = compiles.clone();
        thread::Builder::new()
            .name("typst-compile".into())
            .spawn(move || {
                let disk = disk_cache.and_then(DiskCache::open);
                run(rx, disk, counter, deliver)
            })
            .expect("spawn compile thread");
        Self { tx, compiles }
    }

    /// Number of actual compiles so far (cache hits do not count).
    pub fn compiles(&self) -> usize {
        self.compiles.load(Ordering::Relaxed)
    }

    pub fn submit(&self, job: CompileJob) {
        // The thread only exits when this handle is dropped.
        let _ = self.tx.send(job);
    }
}

/// A cached compile: raster plus id-free geometry.
struct Cached {
    raster: Arc<Raster>,
    raw: Arc<RawLayout>,
}

/// Bumped when the geometry extracted from a render changes, so layouts
/// cached on disk by older builds are not reused. 2: generated text (an
/// equation's number) no longer counts as the nodes after its element's start.
const LAYOUT_VERSION: u32 = 2;

fn cache_key(job: &CompileJob) -> u64 {
    let mut h = DefaultHasher::new();
    LAYOUT_VERSION.hash(&mut h);
    job.around.hash(&mut h);
    job.snippet.hash(&mut h);
    job.width_pt.to_bits().hash(&mut h);
    job.pixel_per_pt.to_bits().hash(&mut h);
    TYPST_VERSION.hash(&mut h);
    h.finish()
}

/// Queue a job, replacing a queued job for the same block.
fn enqueue(queue: &mut VecDeque<CompileJob>, job: CompileJob) {
    match queue.iter_mut().find(|j| j.block == job.block) {
        Some(old) if old.generation <= job.generation => *old = job,
        Some(_) => {}
        None => queue.push_back(job),
    }
}

fn run(
    rx: Receiver<CompileJob>,
    disk: Option<DiskCache>,
    compiles: Arc<AtomicUsize>,
    deliver: impl Fn(CompileResult),
) {
    let mut world = BridgeWorld::new();
    let mut cache: HashMap<u64, Cached> = HashMap::new();
    let mut queue = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(job) => queue.push_back(job),
                Err(_) => return,
            }
        }
        while let Ok(job) = rx.try_recv() {
            enqueue(&mut queue, job);
        }
        let Some(job) = queue.pop_front() else {
            continue;
        };

        let key = cache_key(&job);
        let start = Instant::now();
        let mut elapsed = Duration::ZERO;
        let on_disk = || {
            let disk = disk.as_ref().filter(|_| job.persist)?;
            let (raster, raw) = disk.load(key)?;
            Some((Arc::new(raster), Arc::new(raw)))
        };
        let outcome = match cache.get(&key) {
            Some(c) => Ok((c.raster.clone(), c.raw.clone())),
            None => {
                let result = match on_disk() {
                    Some(hit) => Ok(hit),
                    None => {
                        compiles.fetch_add(1, Ordering::Relaxed);
                        let result =
                            catch_unwind(AssertUnwindSafe(|| compile_job(&mut world, &job)))
                                .unwrap_or_else(|_| Err(vec!["internal compiler panic".into()]));
                        elapsed = start.elapsed();
                        if let (Ok((raster, raw)), Some(disk)) = (&result, &disk)
                            && job.persist
                        {
                            disk.store(key, raster, raw);
                        }
                        result
                    }
                };
                result.inspect(|(raster, raw)| {
                    if job.transient {
                        return;
                    }
                    if cache.len() >= CACHE_CAPACITY {
                        cache.clear();
                    }
                    cache.insert(
                        key,
                        Cached {
                            raster: raster.clone(),
                            raw: raw.clone(),
                        },
                    );
                })
            }
        };
        let outcome = outcome.map(|(raster, raw)| Rendered {
            raster,
            layout: raw.resolve(&job.map),
            pixel_per_pt: job.pixel_per_pt,
            end_state: raw.end_state.clone(),
            targets: raw.targets.clone(),
        });
        deliver(CompileResult {
            block: job.block,
            generation: job.generation,
            outcome,
            elapsed,
        });

        if queue.is_empty() {
            // Drop memoized results that haven't been used for a while.
            typst::comemo::evict(30);
        }
    }
}

fn compile_job(
    world: &mut BridgeWorld,
    job: &CompileJob,
) -> Result<(Arc<Raster>, Arc<RawLayout>), Vec<String>> {
    let (src, offset) = block_document(&job.around, &job.snippet, job.width_pt);
    world.set_files(job.around.files.clone());
    let doc = compile(world, &src)?;
    let page = doc
        .pages()
        .first()
        .ok_or_else(|| vec!["no page".to_string()])?;
    let raster = render_page(page, job.pixel_per_pt);
    let mut raw = raw_layout(world, page, offset);
    raw.end_state = counters::end_state(&doc);
    raw.targets = counters::label_targets(&doc);
    Ok((Arc::new(raster), Arc::new(raw)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize_block;
    use model::Document;

    fn job(block: NodeId, generation: u64, snippet: &str) -> CompileJob {
        CompileJob {
            block,
            generation,
            around: Around::default(),
            snippet: snippet.into(),
            map: SpanMap::default(),
            width_pt: 200.0,
            pixel_per_pt: 1.0,
            persist: false,
            transient: false,
        }
    }

    /// Persisted renders survive a new worker (an app restart): no compile.
    #[test]
    fn disk_cache_serves_persisted_renders() {
        let dir = std::env::temp_dir().join(format!("sem-disk-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut d = Document::empty();
        let g = d.diagram("circle((0, 0))\nline((0, 0), (2, 1))");
        let (snippet, map) = serialize_block(&g);
        let job = CompileJob {
            map,
            persist: true,
            transient: false,
            ..job(g.id, 1, &snippet)
        };
        let mut layouts = vec![];
        for round in 0..2 {
            let (tx, rx) = mpsc::channel();
            let worker = Worker::spawn(Some(dir.clone()), move |r| tx.send(r).unwrap());
            worker.submit(job.clone());
            let result = rx.recv_timeout(Duration::from_secs(60)).unwrap();
            let rendered = result.outcome.expect("diagram renders");
            assert!(rendered.raster.width > 0);
            layouts.push(rendered.layout);
            assert_eq!(
                worker.compiles(),
                if round == 0 { 1 } else { 0 },
                "round {round}"
            );
        }
        assert_eq!(layouts[0], layouts[1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enqueue_replaces_superseded_jobs() {
        let mut q = VecDeque::new();
        enqueue(&mut q, job(NodeId(1), 1, "a"));
        enqueue(&mut q, job(NodeId(2), 1, "b"));
        enqueue(&mut q, job(NodeId(1), 3, "c"));
        enqueue(&mut q, job(NodeId(1), 2, "stale"));
        let got: Vec<_> = q
            .iter()
            .map(|j| (j.block.0, j.generation, j.snippet.as_str()))
            .collect();
        assert_eq!(got, [(1, 3, "c"), (2, 1, "b")]);
    }

    #[test]
    fn worker_compiles_reports_errors_and_caches() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, move |r| tx.send(r).unwrap());
        let mut d = Document::empty();
        let x = d.ident("x");
        let xid = x.id;
        let block = d.display(vec![x]);
        let (snippet, map) = serialize_block(&block);
        let ok = CompileJob {
            map,
            ..job(block.id, 1, &snippet)
        };
        let recv = || rx.recv_timeout(Duration::from_secs(30)).unwrap();
        worker.submit(ok.clone());
        let first = recv();
        let rendered = first.outcome.as_ref().unwrap();
        assert!(rendered.layout.box_of(xid).is_some());
        assert!(rendered.raster.width >= 200);

        worker.submit(job(NodeId(99), 1, "#nonsense()"));
        worker.submit(CompileJob {
            generation: 2,
            ..ok
        });
        let mut results = [recv(), recv()];
        results.sort_by_key(|r| r.block);
        assert_eq!(results[0].generation, 2);
        assert!(results[0].outcome.is_ok());
        assert_eq!(results[0].elapsed, Duration::ZERO, "cache hit");
        assert!(results[1].outcome.is_err());
    }
}
