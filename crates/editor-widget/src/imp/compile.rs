//! Compile scheduling: block jobs, the preview, outline entries and
//! snippets, and what to do with their results.

use super::*;

impl SemEditor {
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
    pub(super) fn submit_blocks(&self, force: bool) {
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
        // Pieces edited since they failed are tried again.
        let failed: Vec<NodeId> = {
            let mut failed = self.failed.borrow_mut();
            failed.retain(|&id, f| jobs::failing_source(ed.doc(), id).as_ref() == Some(&f.source));
            failed.keys().copied().collect()
        };
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
                jobs::Sources {
                    open: ed.open_code(),
                    failed: &failed,
                },
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

    /// The header snippets are rendered with: the last block's, which holds
    /// the code blocks' definitions too.
    pub(super) fn snippet_header(&self) -> Arc<str> {
        match self.views.borrow().last() {
            Some(view) => view.around.header.clone(),
            None => self.editor.borrow().doc().header.as_str().into(),
        }
    }

    /// Render Typst `markup` (not part of the document) `width_pt` wide,
    /// with the definitions of the whole document in force, in colour `c`
    /// at 11pt; `done` gets the texture cropped to the ink, or the errors.
    pub(crate) fn render_snippet(
        &self,
        markup: &str,
        width_pt: f64,
        c: gdk::RGBA,
        done: SnippetDone,
    ) {
        let header = self.snippet_header();
        let files = self.editor.borrow().doc().files.clone();
        let hex = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = format!(
            "rgb(\"#{:02x}{:02x}{:02x}\")",
            hex(c.red()),
            hex(c.green()),
            hex(c.blue())
        );
        let prefix = snippet_rule(&color);
        let n = self.next_snippet.get() + 1;
        self.next_snippet.set(n);
        let id = n | SNIPPET_BIT;
        self.snippets.borrow_mut().insert(id, done);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.worker.get().expect("worker").submit(CompileJob {
            block: NodeId(id),
            generation,
            around: Around {
                header,
                prefix,
                files,
                ..Around::default()
            },
            snippet: markup.to_string(),
            map: SpanMap::default(),
            width_pt,
            pixel_per_pt: SNIPPET_PX_PER_PT * self.obj().scale_factor() as f64,
            persist: false,
            transient: true,
        });
    }

    fn on_snippet_result(&self, result: CompileResult) {
        let Some(done) = self.snippets.borrow_mut().remove(&result.block.0) else {
            return;
        };
        done(
            result
                .outcome
                .map_err(|e| e.join("\n"))
                .and_then(|rendered| {
                    let raster = rendered.raster;
                    let bounds = raster.ink_bounds().ok_or("nothing to show")?;
                    let scale = rendered.pixel_per_pt / SNIPPET_PX_PER_PT;
                    Ok((texture(Arc::new(raster.crop(bounds))), scale))
                }),
        );
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

    pub(super) fn on_result(&self, result: CompileResult) {
        if result.block == PREVIEW_ID {
            self.on_preview_result(result);
            return;
        }
        if result.block.0 & SNIPPET_BIT != 0 {
            self.on_snippet_result(result);
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
            let mut blamed = false;
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
                    if latest {
                        blamed = self.blame(view, &errors, &result.error_at);
                    }
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
            current || retargeted || blamed
        };
        if heading {
            self.submit_outline_entry(result.block);
        }
        if chained {
            // The next counter-sensitive block can go now; a block with a
            // failed piece goes again, the piece showing its source.
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

    /// Blame the failed compile of `view` (`errors`, each `at` a place in
    /// its snippet) on the code pieces and calls there; true if one was
    /// newly blamed (then the block shows it as source).
    fn blame(&self, view: &BlockView, errors: &[String], at: &[Option<usize>]) -> bool {
        let ed = self.editor.borrow();
        let mut failed = self.failed.borrow_mut();
        let mut blamed = false;
        for (message, at) in errors.iter().zip(at) {
            let Some(id) = at.and_then(|at| view.map.innermost(at)) else {
                continue;
            };
            let Some(source) = jobs::failing_source(ed.doc(), id) else {
                continue;
            };
            if failed.get(&id).is_none_or(|f| f.source != source) {
                blamed = true;
            }
            failed.insert(
                id,
                Failed {
                    block: view.id,
                    source,
                    message: message.clone(),
                },
            );
        }
        blamed
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
