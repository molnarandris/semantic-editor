//! Drawing on top of the block renders: outlines, selection, remarks,
//! label pills, tabs, formula marks.

use super::*;

impl SemEditor {
    /// Current structure and selection, drawn under the text.
    /// A faint outline around the environment the caret is in (tinted) or
    /// just before/after (outline only), so in and out can be told apart.
    pub(super) fn snapshot_env_outline(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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
    pub(super) fn snapshot_call_outline(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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
    pub(super) fn snapshot_selection(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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
    pub(super) fn snapshot_remarks(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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
    /// or a block's bar): the text of each.
    pub(super) fn remark_at(&self, x: f64, y: f64) -> Option<String> {
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
                found.push(remark.text.clone());
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
    /// `first` to `last` (in one block, or from one to a later one), in
    /// widget coordinates.
    fn range_rects(
        &self,
        first: &CursorPath,
        last: &CursorPath,
        x0: f64,
        scroll: f64,
    ) -> Vec<graphene::Rect> {
        let (i, j) = {
            let ed = self.editor.borrow();
            let doc = ed.doc();
            match (doc.top_index(first.block), doc.top_index(last.block)) {
                (Some(i), Some(j)) => (i, j),
                _ => return vec![],
            }
        };
        (i..=j)
            .flat_map(|index| {
                let from = (index == i).then_some(first);
                let to = (index == j).then_some(last);
                self.block_range_rects(index, from, to, x0, scroll)
            })
            .collect()
    }

    /// [`Self::range_rects`] in top-level block `index`: from `first` (or
    /// its start) to `last` (or its end).
    fn block_range_rects(
        &self,
        index: usize,
        first: Option<&CursorPath>,
        last: Option<&CursorPath>,
        x0: f64,
        scroll: f64,
    ) -> Vec<graphene::Rect> {
        let zoom = self.zoom.get();
        let Some(&top) = self.block_tops().get(index) else {
            return vec![];
        };
        let stops = self.block_stops(index);
        let find = |c: Option<&CursorPath>, default: usize| match c {
            Some(c) => stops.iter().position(|s| s.cursor == *c),
            None => Some(default),
        };
        let (Some(a), Some(b)) = (find(first, 0), find(last, stops.len().saturating_sub(1))) else {
            return vec![];
        };
        if stops.is_empty() {
            return vec![];
        }
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
    pub(super) fn tab_call(&self, scroll: f64) -> Option<TabCall> {
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
    pub(super) fn snapshot_math_call_outline(
        &self,
        snapshot: &gtk::Snapshot,
        x0: f64,
        scroll: f64,
    ) {
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
    pub(super) fn snapshot_call_tab(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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

    /// A light green background behind every reference and hyperlink in
    /// view (a wrapped link gets one box per line), so links stand out
    /// whatever the header's styling (editor only, not in Typst).
    pub(super) fn snapshot_ref_marks(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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
    pub(super) fn snapshot_label_pills(
        &self,
        snapshot: &gtk::Snapshot,
        x0: f64,
        scroll: f64,
        focus: bool,
    ) {
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
    pub(super) fn snapshot_formula_marks(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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

    pub(super) fn snapshot_highlights(&self, snapshot: &gtk::Snapshot, x0: f64, scroll: f64) {
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
}
