# Implementation plan

This file is the running record of what was built and how (per milestone, plus
follow-ups between milestones). `spec.md` holds the goals and the wishlist of
upcoming milestones (M8+). Both are updated with every change.

Pinned: Typst **0.15.1** (`typst`, `typst-layout`, `typst-render`, `typst-pdf`, `typst-kit`),
gtk4-rs 0.11, libadwaita 0.9, sourceview5 0.11. Rust 2024 edition.

Decisions on the spec's open questions (change any time):

- Fonts: the embedded Typst fonts only (New Computer Modern + NCM Math, Libertinus,
  DejaVu Sans Mono) via `typst-kit/embedded-fonts`. Deterministic, no fontconfig.
  System fonts can be added later behind the same `World`.
- `typst-layout` is added as a direct dependency: `PagedDocument` lives there and
  is not re-exported by `typst`. Same pinned version; Typst-only, inside `typst-bridge`.
- `today()` returns `None` for now (no `chrono`).

## Pitfalls: sharp rendering (read before touching drawing code)

Learned the hard way (2026-11-08, blurry text that Typesetter did not have):

- Draw rasters with `snapshot.append_texture`, never `append_scaled_texture`: GTK's
  scaled-texture node resamples even at exactly 1:1 (GL and Vulkan alike), which
  smears thin glyphs. Typesetter uses plain texture nodes too.
- Render at `zoom × device scale` and draw the texture at its own size in device
  pixels, at a whole-device-pixel position. Any mismatch resamples.
- Not causes (measured): the font or a missing anti-aliasing setting (`typst_render`
  has none; our raster dumped to PNG is crisp), debug vs release builds, transparent
  block pages (GTK blends in sRGB, like tiny-skia on an opaque page).
- How to debug: dump the raster as PNG and compare it with the pixels on screen
  (screenshot, magnified). Render a node offscreen with
  `Gsk.*Renderer.realize_for_display` + `render_texture` (PyGObject in the SDK, no
  window) to see what GTK does to it. Measure before adding code; when matching
  Typesetter is an option, do that.

## Build and packaging (Flatpak only)

App ID `io.github.molnarandris.SemanticEditor` (`.Devel` suffix for dev builds), GNOME
Platform/SDK 51 + `rust-stable//26.08`; gtk4-rs with `v4_18`, libadwaita with `v1_8`.

- Manifest: `build-aux/io.github.molnarandris.SemanticEditor.Devel.json` (GNOME Builder
  picks it up). Network allowed during build for now; Flathub later needs
  `cargo-sources.json` from `flatpak-cargo-generator`.
- Offline builds: Builder's play button ignores `build-commands` and runs its own
  `cargo build` in a sandbox where `~/.cargo` is an empty tmpfs, so the manifest env sets
  `CARGO_HOME=target/cargo-home` (relative to the source dir; Builder honours
  `build-options.env`). Cargo needs every crate in `Cargo.lock` cached, other platforms'
  included, or it tries the network: `build-commands` run `cargo fetch` first; for Builder,
  after dependency changes run once online:
  `build-aux/sdk sh -c 'CARGO_HOME=target/cargo-home cargo fetch'`.
- CLI loop: `build-aux/sdk cargo test|clippy|run` runs cargo inside the SDK
  (target dir `target/sdk`).
- Install: `flatpak-builder --user --install --force-clean build-dir build-aux/*.Devel.json`.
- The app ID is baked in at compile time from `APP_ID` (set in the manifest).

## M1 — Benchmark spike

- `typst-bridge::world::BridgeWorld`: `Library::default()`, `FontBook` from the embedded
  fonts (loaded once), one in-memory main `Source` that is replaced per compile
  (`Source::replace` keeps comemo/incremental benefits for warm compiles).
- `typst-bridge::compile_block(source, width_pt) -> Result<PagedDocument, Vec<String>>`
  wraps the snippet in the per-block preamble (`#set page(width, height: auto, margin: 0pt)`).
- `render_page(page, pixel_per_pt) -> Pixmap` via `typst-render`.
- `bench` binary (`crates/typst-bridge/src/bin/bench.rs`): 5 formulas; world creation time,
  cold compile+render per formula, then warm (N=20, median) compile and render separately.

## M2 — Model and serializer

- `model`: `NodeId` allocator, node structs wrapping the enums of the spec, `Document`,
  `CursorPath`/`Step`, `Selection`, `Op` + `apply(op) -> inverse`, `History` (undo/redo).
  Ops address nodes by parent path + index so they're invertible without GTK.
- Tests: op/inverse round trip on random-ish documents, cursor validity after each op.
- `typst-bridge::serialize(block) -> (String, SpanMap)`; `SpanMap` = sorted
  `(Range<usize>, NodeId)` with innermost-lookup. Escape text; quote multi-letter idents;
  slot box. Golden snapshot tests (plain `assert_eq!` strings, no new deps) and
  "every snapshot compiles" test.

## M3 — Viewer

- `typst-bridge::worker`: thread owning `BridgeWorld`, job channel, drop superseded
  jobs per block (latest generation wins), cache keyed by hash(source, width, version, scale).
- `BlockLayout` from walking `Frame` items: glyph spans → byte offset → `NodeId` boxes.
- `editor-widget::SemEditor`: custom widget + `Scrollable`; textures via `gdk::MemoryTexture`.
- `app`: `adw::Application`, header bar, hard-coded sample doc, status bar stub.

Implementation notes (done):

- Every block renders on a page `BLOCK_PAD_PT` (8 pt) wider/taller on each side with
  `fill: none`, so descenders and big operators are not clipped. Layout coordinates are
  relative to the content box; the widget draws the texture shifted by the pad and
  stacks blocks by content height.
- `RawLayout` (geometry keyed by snippet byte offsets, no node ids) is what the worker
  caches; `RawLayout::resolve(&SpanMap)` maps it to `BlockLayout` per job. Two blocks
  with identical source can therefore share a cached render safely.
- `sem-slot` is defined as `box.with(..)` so each slot's frame carries its own call-site
  span; a closure would give all slots the preamble's span.
- Results hop to the GTK main loop via `glib::SendWeakRef` + `MainContext::invoke`
  (no extra channel crate).
- Debug: `SEM_SCREENSHOT=/path.png` saves a PNG of the window after 1.5 s and quits.

## M4 — Prose editing

Text caret from glyph boxes (per-cluster x positions), click → nearest char, typing via
`IMMulticontext`, Backspace, Enter (split), arrows, undo.

Implementation notes (done):

- `model::Editor` owns document + caret + history and turns commands (insert text,
  Backspace, Delete, Enter, ←/→, undo/redo) into ops. Typing and single-char deletes
  merge into one undo step per word (`History::apply_merged`).
- `editor-widget::caret` (no GTK): every caret position of a block ("stops") with x and
  line from the layout. Ligatures are split evenly; chars without a glyph (collapsed or
  trailing spaces) are placed after their neighbour. Clicks pick the nearest line, then
  the nearest stop; ↑/↓/Home/End use the same lines with a sticky goal column.
- No debounce: the worker already drops superseded jobs and a paragraph compiles in a
  few ms. Until the new render arrives the caret uses the previous layout.
- Spaces Typst would collapse (doubled, leading a paragraph) are never stored: typing
  skips them, and deletes and joins remove the extra one. The caret blinks without a timeout.
- Not yet: selection, typing inside math (formulas are passed over as one step), editing
  raw Typst blocks in the canvas, preedit display for input methods.

## M5 — Math editing

Structural keys per the table, slots, symbol shortcut table, breadcrumb, structure
highlight. If glyph spans turn out too coarse, use the marker-box fallback from the spec
(serializer wraps nodes in labelled boxes, layout reads their frames).

Implementation notes (done; spans were fine-grained enough, no marker boxes):

- Input follows Typst where possible. Letters form one identifier until a space or
  another key ends the word; finished words in the symbol table become symbols (`chi` →
  χ, `\alpha` → α, TeX names only with `\`). Digits merge into numbers. Operator pairs
  combine (`<=` → ≤, `->` → →). Words are looked up in Typst's own symbol table
  (`typst_bridge::resolve_symbol`, handed to the model as a plain `fn`, so `model` stays
  Typst-free); `.` continues a symbol name with modifiers (`times.o` → ⊗). TeX names
  need a `\`. Operators (`log`, `lim`) are emitted as `op(#[log])` so their glyphs map
  back to the node.
- `/` takes the selection, a `( … )` group (dropping the parens, like Typst) or the
  previous operand as numerator; with none, an empty fraction. `^`/`_` likewise
  (a group keeps its parens as base). `(` typed into an empty slot only opens a group;
  its `)` leaves the structure. So `S <= (log chi)/(2) + c` types as written.
- ←/→ walk `Document::positions` (every caret position in reading order; structures
  are entered and left). ↑/↓ go numerator ↔ denominator and base ↔ scripts in the
  innermost structure, otherwise by line. Tab / Shift+Tab: next / previous empty slot.
- Backspace after a structure or inline formula selects it, the second press deletes.
  In an empty slot it removes the slot and unwraps (fraction → the other part, scripts
  → base). Escape leaves the structure, then the formula; `$` leaves the formula, and
  `$$` turns an empty inline formula into a display block.
- Widget: caret stops for every row (left edge of each node, middle of an empty
  slot); clicks weigh distance to each row so numerator and denominator are reachable.
  The formula and innermost structure are tinted, the selection filled. Status bar:
  clickable breadcrumb (selects that structure) and a key hint.
- Tests: the acceptance sequence key by key, every intermediate state compiles, and a
  keystroke fuzz test (invariants, caret validity, undo to start).

Follow-ups after M5 (done):

- Paired brackets `( [ {` … `) ] }` matched within a row are emitted literally so
  Typst stretches them; unpaired ones stay as names. Glyph boxes grow to the ink
  bounds, so stretched delimiters and big operators get their real height for the
  caret and highlights.
- Headings: typing `=`…`======` + space at the start of a paragraph makes it a heading
  of that level (as in Typst); Ctrl+1…6 set the level, Ctrl+0 makes it a paragraph.
  Backspace at the start of a heading first turns it into a paragraph, the next press
  joins with the block before. The status hint mentions `= ` + space.

## M6 — Files and export

`.sem` JSON, NodeIds regenerated on load, `.typ` and PDF export, sourceview5 pane,
GActions + accelerators. Test: save → load → export identical Typst.

Implementation notes (done):

- `app::file`: `{ "version": 1, "typst": "0.15.1", "blocks": [...] }`, pretty-printed.
  Load regenerates ids and normalizes; newer format versions are refused. Tests:
  save → load → export gives identical Typst and an identical file; export compiles
  to PDF.
- `app::window`: one `Window` per document (state kept alive by a thread-local list,
  closures hold weak refs). Unsaved changes = blocks differ from the last saved/loaded
  ones (so undoing back to the saved state is clean); title shows `•`, closing / opening
  asks Save / Discard / Cancel (`adw::AlertDialog`). File dialogs are `gtk::FileDialog`
  through the portal, IO is async (`gio::File` futures).
- Two serializer styles. `Style::Edit` (per-block renders) keeps every glyph's span in
  its node: always-parenthesized groups, `op(#[log])`. `Style::Readable` (export,
  source pane): `1/2`, `(a + b)/2`, `x^2`, `x^(n + 1)`, `(a b)^2`, plain `log`, literal
  `,`/`;` (not inside `attach` args), `f(x)` for single letters. A test renders both
  styles for many formulas and every typing state and requires identical pixels (this
  caught `liminf`: Typst's text has a U+2009 thin space).
- PDF: `typst_bridge::compile_pdf` (own world) on `gio::spawn_blocking`. `.typ`: the
  serializer output. Errors and success show as toasts.
- Source pane: `adw::OverlaySplitView` with a read-only `sourceview5::View` (F9 or the
  header button). It follows the caret: the source cursor sits at the caret's position
  and the innermost structure is tinted (`serialize_document_mapped`). No Typst syntax
  definition in GtkSourceView 5.22, so plain monospace text for now.
- Actions: `app.new` Ctrl+N (new window), `app.quit` Ctrl+Q, `win.open` Ctrl+O,
  `win.save` Ctrl+S, `win.save-as` Ctrl+Shift+S, `win.export-pdf` Ctrl+E,
  `win.export-typ` Ctrl+Shift+E, `win.source-pane` F9, close Ctrl+W. The app starts
  with an empty document; the M3 sample is in the menu ("Open Sample Document").
  `HANDLES_OPEN`: files passed on the command line open in their own windows.

Follow-ups after M6 (done):

- Source pane: GTK draws a text view's cursor only when focused, so a small
  `sourceview5::View` subclass draws a caret bar at the mirrored position otherwise.
  (Removed in M8.)
- Block textures are drawn one texture pixel per device pixel, snapped to the device
  pixel grid relative to the surface: fractional placement made GTK resample and
  soften the glyphs.
- Blurry text, fixed 2026-11-08: block textures were drawn with
  `append_scaled_texture`, and GTK's scaled-texture node resamples even at exactly 1:1
  (checked offscreen with the GL and Vulkan renderers at scale 2: up to 176/255 off).
  A plain `append_texture`, as Typesetter does, is pixel-exact. Used in the editor and
  the block preview.
- Zoom: Ctrl+wheel, touchpad pinch, Ctrl+plus / Ctrl+minus (handled in the widget;
  Ctrl+0 stays "make paragraph"), and menu entries incl. Reset Zoom (`win.zoom-in`, `win.zoom-out`, `win.zoom-reset`). Default 1.75×, range
  0.75×–5×, step 1.1×. Blocks re-render at the new scale (scale is in the render key);
  until then the old texture is scaled with linear sampling.

## M7 — CeTZ diagrams

Implementation notes (done):

- Packages: `BridgeWorld` serves package files like the Typst CLI (`typst-kit`
  `system-packages` + `system-downloader`): package data dir, cache dir, else download
  from Typst Universe into the cache (in the Flatpak: `~/.var/app/<id>/cache/typst`).
  The Flatpak has `--share=network` for this. `tiny-skia` (already in the build via
  `typst-render`) is a direct dependency only for PNG encode/decode.
- `Block::Diagram(source)`: the body of `cetz.canvas({ import cetz.draw: * … })`,
  centred. CeTZ pinned to 0.5.3 (`CETZ_VERSION`); block snippets import it, the
  exported document imports it once at the top.
- Editing: Insert Diagram (Ctrl+Shift+D) / Insert Raw Typst from the menu; Enter or
  double click on such a block opens its source in the side panel (editable
  GtkSourceView, compile error shown below). Edits apply 300 ms after the last key
  (`Editor::set_block_source`, one undo step per block while editing); the page is the
  preview. Esc / Done goes back. Backspace/Delete on the block select it, then delete.
  Escape on the block moves to (or creates) the paragraph after it.
- Caching: memory as before; jobs flagged `persist` (diagram and raw blocks) also go
  to a disk cache (PNG + layout JSON per render key, ~/.cache/semantic-editor/renders,
  oldest pruned beyond 1000 files). `jobs::stale_blocks` decides what to submit (only
  changed sources). Tests: 20 diagrams next to edited prose → no diagram recompile;
  a fresh worker serves a persisted render with zero compiles.
- Tests that compile CeTZ need network on their first run (package download).

## Code review after M7 (2026-11-06)

Overall the layering held up (model stays GTK/Typst-free, all Typst in the bridge, caret
geometry testable without GTK). Changes made:

- `model::editor` split from one 2,270-line file into `editor/` modules: `mod.rs`
  (state, commit, selection, status), `input.rs` (typing in text and math),
  `delete.rs`, `structure.rs` (leaving formulas/blocks, Enter, headings, raw/diagram
  sources), `motion.rs`, `tests.rs`. Repeated patterns became helpers: `select`,
  `select_node`, `node_before`, `type_node`, `retype_node_before`.
- Bug: a block was recompiled only when its snippet text changed. Same text with new
  node ids left a layout pointing at dead ids (caret unplaceable). The widget now
  compares the span map too (a worker cache hit, re-resolved against the new ids).
  Test: `new_ids_with_same_source_are_stale`.
- Caret stops are cached per block view (reset when the block is resubmitted or a
  render arrives) instead of rebuilt on every draw, including each caret blink.
- `SemEditor::run` no longer builds and discards a bool per command.
- The CeTZ version is written once (`cetz_version!` macro feeds both constants).

Deliberately left for the milestone that needs it:

- `app::window` mixes window, file actions, source pane and block editor. (Done in M8.)
- Page geometry (`page_x`, `block_tops`, `content_height`, margins) is spread over
  `imp.rs`. M9b (header banner) and M10 (label pills in the margin) add to it; introduce
  a small geometry type then.
- Every command serializes every block to find stale ones, and `is_dirty` compares all
  blocks: O(document) per keystroke, fine at current sizes. M9a's counter chaining
  changes what a block's render depends on, so revisit there.
- `CursorPath` / `RowPos.base` assume blocks are top level. M11 (environments holding
  blocks) is where nesting gets designed.

## M8 — Cleanup and diagram editor

Implementation notes (done):

- Source pane removed (F9, header toggle, `source_view.rs`, the `OverlaySplitView`).
  `serialize_document_mapped` stays (export uses it; its span map is tested).
- `app::window` is now a module: `mod.rs` (window, actions, title, status bar),
  `files.rs` (open/save/export), `block_editor.rs` (the source dialog).
- Diagram and raw Typst blocks are edited in an `adw::Dialog` (1040×640): GtkSourceView
  on the left, live preview on the right (`gtk::Paned`), compile error under the
  preview. A fresh dialog per open; Done or Esc closes it. Edits still apply 300 ms
  after the last key (one undo step per block); pending edits are applied before
  save, export, closing the window and discarding.
- Preview: its own render of the block, cropped to the ink (`Raster::ink_bounds` /
  `crop`) and scaled to fill the panel, centred. `SemEditor::set_preview(block, ppp)`
  submits it to the same worker (job id `NodeId(u64::MAX)`, resubmitted when the
  block's snippet or the page width changes; `preview-rendered` signal). The preview
  widget asks for a re-render whenever the scale it draws at is more than 4 % off
  the rendered one (dialog/pane resize), so it stays sharp; until then the old
  render is scaled. Capped at 12 device px/pt. Such jobs are `transient`: never
  added to the worker's memory or disk cache, so renders at arbitrary scales don't
  pile up. After a failed compile the preview keeps the last good render.
- The source wraps (`WordChar`, no horizontal scroll) with a hanging indent: each
  indented line gets a tag `indent-N` with `indent = -N × space width`, so wrapped
  continuation lines start under the line's first non-blank character. Tags are
  recomputed on every buffer change (block sources are short).
- The dialog closes itself when its block disappears (undo of the insert, new
  document) and reloads its text when the source changes from outside.

## M9a — Header and numbering engine

Implementation notes (done):

- `Document.header: String` (model). Edited with `Editor::set_header`, an
  `Op::Replace` on the new `Seq::Header` (the same text op as raw sources), so
  consecutive edits are one undo step and the caret stays. `.sem` format version 2
  stores it; version 1 files load with an empty header (they look as before).
  New documents get `DEFAULT_HEADER` (`#set heading(numbering: "1.1")`); the sample
  numbers equations too.
- The editor's own preamble (NCM fonts, `sem-slot`) stays and comes first, so a header
  can override the font. Export: preamble, header, blocks.
- `typst_bridge::Around { header, prefix, suffix }`: the code compiled around a block's
  snippet, part of the worker's cache key. `block_document` writes preamble, header,
  then the editor's `#set page(...)` (after the header, so its paper, margins,
  page header/footer do not apply to blocks), prefix, snippet, suffix.
- Counter chaining (`typst_bridge::counters`): tracked counters are `heading`,
  `math.equation` and every `counter("…")` the header names (`tracked_counters`). The
  prefix is one `#counter(…).update((…))` per tracked counter, the suffix
  `#context [#metadata((…get(), …)) <sem-end>]`; `end_state` reads it back from the
  introspector. The end state is stored in `RawLayout`, so cached renders (memory and
  disk) keep it. Tests: chained end states equal a full-document compile (headings,
  numbered equations, a custom counter), and the prefix/suffix change neither size nor
  baseline of a block.
- Scheduling (`editor_widget::jobs::stale_blocks`): headings, display math and raw
  Typst are counter-sensitive. A sensitive block's `Around` holds its start state, so
  a changed start makes it stale. It is submitted only once the previous sensitive
  block's end state is known (its result arrived); the result handler then calls
  `submit_blocks` again. A changed end state ripples down one compile at a time; an
  unchanged one stops it. A failed block passes its start state on. Paragraphs and
  diagrams never wait and keep their cache entries; a header change recompiles all.
  A forced re-render (zoom, page width) clears the views' submitted snippets instead of
  passing a flag, so blocks that wait for the chain are re-rendered when it reaches
  them (`stale_blocks` lost its `force` parameter).
- Numbered headings need nothing extra: the number's glyphs map to the heading block,
  not to a text node, so they get no caret stops (test `heading_numbers_are_not_text`).
- Header editing: the source dialog's target is `PreviewTarget::Block(id)` or
  `::Header` (menu "Document Header…", `win.edit-header`). The header's preview is
  `HEADER_SPECIMEN` (headings, text, inline and display math) under the header. The
  dialog shows the preview's own compile error (`preview_error`; the preview now
  reports failures), which replaced `block_error` and the `block-rendered` signal.
- Dirty tracking compares header and blocks.

Each milestone: `cargo test`, `cargo clippy` clean, then stop for manual testing.
