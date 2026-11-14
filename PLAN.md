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
  A space typed right before an existing one is held (2026-11-10, user report): it goes
  in with the text or formula typed next at the caret, so `foo| bar` + ` text` gives
  `foo text bar`; any caret move drops it.
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
- Follow-up: in a numbered heading without text the caret sat before the number (no
  glyphs, so the fallback stop was at x = 0). The fallback stop now goes after the
  block's visible box (the number) plus a space; an empty paragraph has no box and
  keeps x = 0.

## M10 — Code blocks

Implementation notes (done):

- No model change: a code block is a `Block::RawTypst` whose top level (parsed with
  `typst::syntax`) is only `let`/`set`/`show`/`import` and comments
  (`typst_bridge::is_code`; `#{ let … }` is local, so not code). A raw block becomes a
  code block, or back, as it is edited. Export is unchanged (verbatim).
- `stale_blocks` accumulates the context: each block's `Around.header` is the header
  plus the code blocks before it (an `Arc<str>`, shared by the blocks between two code
  blocks, so the per-keystroke cost stays small). Editing a code block recompiles the
  blocks after it, nothing before. Code blocks are not counter-sensitive.
- Display, drawn by Typst (no widget code): the code block's `Around.suffix` is
  `code_display`: a slim dashed bar naming what it defines (`{ }  let ct, sl · set
  heading`), or with "Show Code" (main menu, `win.show-code`, `SemEditor::set_show_code`)
  the source as a `raw(lang: "typ")` listing. Explicit styles keep the document's rules
  from restyling it much. Caret, clicks, double click → source dialog (titled "Code"),
  and error markers work as for any raw block. Toggling re-renders only code blocks
  (their suffix changes).
- Counter chain rework: `CounterState` is a list of (Typst expression, value). It starts
  with heading, equation and every `counter("…")` in header + code blocks; the suffix
  adds `counter(figure.where(kind: …))` for every figure kind the block contains
  (`query(figure)`, values via `eval`). So theorems built as figures of their own kind
  (as in real papers) are numbered on without the editor knowing the kinds. The test
  against a full-document compile now includes two theorem kinds.
- Sample: a code block defines a `theorem` environment, used by the raw block after it.
- A failing code block makes the blocks after it fail too (they include its code); their
  last good renders stay, with error markers.
- Follow-up: Enter on a raw/diagram/code block no longer opens the source dialog (there
  was no way to add a paragraph after the last such block). Enter at its end adds a
  paragraph after it, at its start one before it (caret stays). Ctrl+Enter or a double
  click opens the dialog (`Command::EditSource`); the status bar hint says so.

## M14a — Environments (theorems, proofs, …) edited in place

Done before M11–M13, at the user's request (2026-11-08). Implementation notes:

- Model: `Block::Env { name, body: Vec<BlockNode> }`, a call `#name[…]` of a function
  from the header or a code block; the body is never empty. The cursor still names a
  leaf block by id (`doc.block(id)` searches at any depth). Block-level editing goes
  through `Place { parent, index }` (`doc.place`, `doc.at`, `doc.sibling`,
  `Place::seq()` = `Seq::Blocks` or the new `Seq::Body(env)`), replacing indexes into
  `doc.blocks`. `start_of`/`end_of(block)` look into environments; `doc.leaves()` is the
  document order for ←/→ across blocks; `top_index(id)` maps to the widget's views.
  Ids, normalization and invariants recurse. `.sem` format version 3.
- Editing: `#name` + space as a whole paragraph makes it an environment (like `= ` for
  headings; "Insert Environment" in the menu starts one by typing `#`). Inside, Enter
  splits paragraphs as usual; Enter on an empty last paragraph moves it out after the
  environment; Escape in its text leaves it; Backspace at the start of its body selects
  the environment (deleted by the next press), or removes it at once if still empty.
  Status bar hints for all of this.
- Rendering: an environment is one top-level view, compiled as one unit, so the
  function's label, number and formatting come out as in the export. Its caret stops
  are its children's, from the same layout (`caret::block_stops`). Counter-sensitive
  (figure kinds are carried by the M10 chain).
- An empty paragraph or heading renders, in the editor's own renders only, a
  transparent `x` at its only caret position: it takes its line, and its glyph puts the
  caret where typed text will appear (after "Lemma 1." in a fresh environment, after a
  heading's number). A zero-width space does not work: Typst drops it when shaping.
- Sample: the theorem and a proof are environments, defined by the code block before.
- Follow-up (user request, TeXmacs-like): an environment has two caret positions of its
  own, just before (offset 0) and just after (offset 1) it, outside it (`Target::Edge`,
  empty path). ←/→ pass through them (`positions` of a top-level block include them;
  `start_of`/`end_of` of an environment are its edges). Esc in its text goes to just
  after it (no new paragraph; it used to create one). At an edge, typing or Enter starts
  a paragraph there, Backspace/Delete select the environment. The widget draws a faint
  rounded outline around the environment the caret is next to, tinted when the caret is
  inside (`Editor::env_focus`, `snapshot_env_outline`); edge stops sit just outside the
  outline on the first/last line, and Home/End skip them (`Stop::edge`).
- Follow-up (user remarks, 2026-11-08):
  - Backspace at the start of an environment's text marks it (`Selected::Unwrap`,
    stronger outline, `Editor::env_marked`); the next Backspace unwraps it: its body
    takes its place, ids kept, so the caret stays. Backspace at an edge still selects,
    then deletes. An empty environment still goes at once.
  - The caret shading of a display equation spans the whole line (the page width, or
    the environment's box), not just the formula, so it is symmetric with a number.
  - `^`/`_` after a large operator (∑, ∏, ∫, ⋃, ⨁, …) attach to it
    (`symbols::is_large_op`); they used to start an empty base. Other operators
    still get an empty base, and `/` still does not take a large operator.
- Not yet (M14b): lists, environments by Typst function detection (any `#identifier`
  is accepted; an unknown one shows a compile error). Both done in M14b.

## M14b — Environment titles, lists, block-level calls

- Titles (2026-11-08). Popular packages checked first: ctheorems, theorion, thmbox and
  frame-it take the title as the first content argument, `#theorem[Title][Body]`
  (great-theorems and theorion also accept `title:`, lemmify `name:`), so that is what
  the editor writes. `Block::Env.title` holds inlines like a paragraph's, empty when
  there is none (serde default, so version-3 files load unchanged). The caret
  addresses it on the environment itself, `[Index(i)]`, between "just before" and the
  body (`Block::text_inlines`, used wherever a caret can type). The sample's `theorem`
  takes `..args` and shows the title as its figure caption: "Theorem 1 (Title)."
  - Typing `#lemma(` starts an environment with the caret in its title; `)` at the end
    of the title (no `(` left open in it) or Enter goes on to the body; Esc goes just
    after the environment. Backspace/Delete at the ends of a title never reach the
    blocks around; Backspace in an empty title removes it. `$$` in a title leaves the
    formula instead of making a display formula.
  - An empty title is shown only in the editor (a transparent `x` keeps the caret
    between the parentheses) and goes, as its own undo step, when the caret leaves it
    (`Editor::drop_left_title`, from `jump` and `set_caret`). `Editor::edit_title`
    adds one (for the click on the heading).
  - A click on an environment's heading ("Theorem 2.1.", which has no caret position)
    edits its title, adding an empty one if needed: the heading is the stretch of the
    first inside line between the outline and the first position inside
    (`caret::heading_at`). The sample's `proof` takes a title too, so clicking "Proof."
    does not give a compile error. With function detection (below) a function known
    to take no title gets none; the click then just places the caret.
- Lists (2026-11-08). Model as in Typst: no list node, a list item is a container
  block, `Block::Item { kind: Bullet | Numbered | Term, term, body }`; consecutive
  items make a list, an item in an item's body is nested. So nothing has to split or
  merge lists. A term item's term is addressed like a title. Items have no caret
  position of their own (no edges, no outline). `Document::env_around` finds the
  environment around a block, so Esc, the outline and Enter-on-empty-last-paragraph
  keep meaning environments only. File format 4.
  - Typst markup in both styles: `- `, `+ `, `/ Term: `; the body's further lines are
    indented two spaces (`Writer::indent`, `nl`, also inside raw/diagram source);
    items are a line apart (a tight list), other blocks a blank line apart. Top-level
    items render one by one, so a numbered one is written `3. …` in the editor's
    renders with its place in the list (`serialize_block_numbered`, counted in
    `stale_blocks`); spacing between top-level items is the editor's block spacing,
    not the list's. Items are counter-sensitive (they may hold equations).
  - Typing (`editor/list.rs`): `- `, `+ `, `/ ` at the start of a paragraph make it an
    item (a term item puts the caret in its term; `:` or Enter at its end go on).
    Enter: the rest of the paragraph and the blocks after it in the item go to a new
    item after it (so a nested list stays below its text). Enter on an empty item: out
    a level (after the item it is nested in, taking the blocks after it along), at the
    outermost level a plain paragraph again; an empty paragraph after an item's first
    block starts a new item. Backspace at an item's start: its blocks continue the item
    before (or stay in place for the first item); after a term, into the term. Tab /
    Shift+Tab (in text; in math they still visit slots): nest in the item before /
    move out a level.
  - Fix (2026-11-09): Shift+Tab takes the kind of the item it moves out of, so a
    numbered sub-item of a bullet list becomes a bullet (term items neither gain nor
    lose their kind).
- Block calls and function detection (2026-11-09). `Block::Env` gained `args`: the
  call's other arguments as Typst source, verbatim, written before the title and body
  (`#align(center)[…]`). File format 5.
  - Detection (`typst_bridge::call_shape`, `calls.rs`): parsing only, no compile, so it
    runs on the keystroke. The top-level `#let`s of the header and the raw blocks (in
    document order, the last wins) give a function's parameters: a body if it has a
    positional parameter or a sink, a title if two positional or a sink. Else Typst's
    global scope: a built-in function takes a body if it has a positional parameter,
    never a title; any other value takes nothing. A name bound to something not written
    as a function (`#let thm = thmbox(…)`), imported from a package or undefined is
    unknown, and is treated as before (body and title allowed). The model gets it as
    `Editor::set_call_resolver` (`CallShape { body, title }`), like the symbol resolver.
  - Typing: `#name ` or `#name[` makes the call; `#name(` a title only if the function
    may take one, else the `(` is typed and `#name(args) ` (balanced parentheses) makes
    a call with those arguments. A name known to take no content (`#pagebreak `) stays
    text. The status bar says which of these applies. `edit_title` refuses a function
    known to take no title.
  - Follow-up (user request): a paragraph that is a call being typed (`#`, `#name`,
    `#name(…`, the whole paragraph) is set in a code font in the editor's renders
    (`typing_call`, `CODE_TEXT` in `serialize.rs`), so it is clear that space or `(`
    will make a call. Export is unchanged (a `#` left in text is written `\#`).
  - Not yet: editing the arguments of an existing call (unwrap with Backspace at the
    start of its body and retype); functions from packages are unknown.

## M11 — Inline code: diagrams inside formulas (2026-11-09)

- Model: `MathKind::Code(String)`, an atom holding Typst code (without the `#`),
  written `#code` in a formula. File format 6. `Document::find_node`/`code_source`
  find it, `Document::top_block` gives the top-level block holding any node (the
  preview renders that block; this also fixes the preview of a diagram nested in an
  environment or list item).
- Editing: "Insert Diagram" with the caret in a formula inserts one at the caret
  (`INLINE_DIAGRAM_TEMPLATE`: `box(baseline: 30%, cetz.canvas(length: 1em, {…}))`),
  caret after it, and opens the source dialog ("Code in a Formula") with the live
  preview of the formula's block. Ctrl+Enter or a double click opens the code atom
  that is selected, else just before the caret, else just after it
  (`Editor::source_target`, used by the widget instead of the caret's block).
  `block_source`/`set_block_source` take a code atom's id too: the atom is replaced
  by one with the same id (one undo step per run of edits, caret stays). Backspace
  and Delete select it first, like a structure. Status bar hint next to one.
- Rendering: in the editor's renders the code is `#sem-code($#code$)`, `sem-code`
  being a `box.with` with a transparent stroke (preamble). CeTZ shapes carry CeTZ's
  spans, so without it an inline canvas has no layout box (measured: no caret stop
  after a trailing diagram, no click target, no selection highlight); the stroke has
  the call site's span. The `$…$` keeps the code in math, as in the file; boxes lay
  out exactly as unwrapped (same x for what follows). The readable style always
  keeps the space after code (a `;` right after `#x` would be read as part of it).
- CeTZ import: a standalone render imports CeTZ when the block has code mentioning
  `cetz` in a formula (a diagram block still imports it itself); the exported file
  imports it once if any block, at any depth, has a diagram or such code (it used to
  check top-level diagram blocks only).
- Follow-up (user request): the dialog for an inline diagram is the diagram editor.
  `MathKind::Diagram(String)` holds only the drawing code, like a diagram block; the
  serializer adds `box(baseline: 30%, cetz.canvas(length: 1em, { import cetz.draw: *
  … }))`. Insert Diagram makes one (`MathKind::Code` stays for other code in
  formulas). The preview renders only the edited item (`serialize_math_node`), also
  for a block nested in an environment or list item (`serialize_block`), with the
  header and code blocks in force; a top-level block still previews its own render.
- In text: type `$` then Insert Diagram, which gives `$#box(…)$`, laid out like the
  box in text. A separate text inline kind was not added: a third inline kind would
  touch every place that relies on text and formulas alternating.
- Fix (2026-11-09, user report): with a numbered display formula the caret ended
  after the number when the formula had one node, and sat in the middle of the line
  in an empty one. Typst gives generated text (the number "(1)") the equation's span
  with a delta counting its own chars, so its third char landed on the formula's
  first node. A glyph's delta now counts only for spans of leaf syntax nodes (source
  text); otherwise it stays at the element's start (`layout.rs`, `span_info`). Same
  for heading numbers longer than the `= ` before the text. `LAYOUT_VERSION` in the
  render cache key, so cached layouts from older builds are not reused.

## M12a — Function calls in math (2026-11-09)

M12 is split: M12a the generic call node, M12b the atoms (primes, text in math, `&`,
`\`).

- Model: `MathKind::Call { name, args: Vec<MathRow> }`, a structure whose child rows
  are its arguments (`Step::Arg(i)`); everything generic over `children()` (positions,
  stops, Tab to empty slots, selection, ops) works through it. File format 7.
- What a name is (`typst_bridge::math_call_shape`, `calls.rs`, shares the `#let`
  parsing with block calls): a function defined in the header or a code block (one
  slot per positional parameter, a sink makes it variadic), else Typst's math scope: a
  function (one slot per positional parameter, named ones like `abs`'s `size` do not
  count; a variadic one, `cases`, starts with one slot), or a symbol that can be called
  (accents and delimiters: `hat`, `tilde`, `floor`; one slot). Plain symbols (`phi`),
  values and single letters (never calls in Typst math) are not calls. The model gets
  it as `Editor::set_math_call_resolver` (`MathCallShape { slots, variadic }`).
- Typing: the word then space makes the call if it is not also a symbol (`sqrt`,
  `cal`, `ket`); `(` makes it always (`hat(`), the `(` absorbed, and `)` leaves it from
  any argument. The caret goes into the first argument. `,` in an argument goes to the
  next one; in the last argument of a variadic call it adds one; otherwise it is a
  comma (written `comma`, so it cannot split the arguments). Backspace in an empty
  argument: all empty → the call goes; an extra argument of a variadic call goes,
  caret to the end of the one before; else ←. Backspace after a call selects it.
  Status bar hint while typing a function's name.
- Rendering: `name(arg, …)` in both styles; Typst draws everything, the arguments'
  glyphs map to their rows, the call's own marks (√, bars) to the call (thanks to the
  generated-text span fix in `layout.rs`, they no longer leak into the first node).
- Not yet: named arguments (`abs(size: …)`), `mat` rows (`;`), choosing an optional
  argument away (`root`'s empty index stays a slot).

## M12b — Atoms in math (2026-11-09)

- Model: `MathKind::Primes(n)`, `Text(String)`, `Align`, `Linebreak`. File format 8.
- Typing: `'` adds a prime (one node per run, `f''` is `Primes(2)`); `"` starts text:
  every key (space, `$`, `,` …) goes into it while the caret stays after it, `"` ends
  it (reusing the word state: `Editor::text_before`). Backspace takes the last char of
  a text (or one prime) back; an empty text goes. `&` is an alignment point; `\` then
  space a line break (as in Typst; `\alpha`-style names are unchanged). Hint while
  typing text.
- Rendering: primes are written right after their base, with no space (measured:
  `f '` does not attach in Typst, `f'` does), except after a fraction (they would
  attach to the denominator) or code. Text is a quoted string; an empty one shows a
  transparent `x` in the editor. `\` is always followed by a space, so it cannot escape
  what comes next.
- Multi-line formulas: `row_stops` splits a row at line breaks; each stop takes its
  line's height and a baseline moved down with the line, and the first stop after a
  break starts at the line's left. So ↑/↓ move between the lines and `&` columns line
  up (test `stops_in_multi_line_formulas`).
- Fix after testing: a break that leaves its line empty (at the end, or before
  another break) is written `\ #sem-slot()` in the editor, inside the break's span,
  so the new line shows a placeholder and the caret after the break sits in it,
  centred like in any empty slot (test `caret_on_an_empty_new_line`).
- Marks for what the render does not show, only on the formula being edited (the
  display formula, or the inline formula holding the caret): a thin line at each `&`
  (between its neighbours' stops) and `↵` after the end of a line with a break.
  Drawn by the widget over the render (`caret::formula_marks`, test
  `marks_of_the_formula_being_edited`), not written into the Typst: measured, a
  zero-width box at `&` changes the spacing around `=`/`+` (it is an ordinary atom),
  a bare `place` too, and drawing in the widget needs no recompile when the caret
  enters or leaves a formula.
- Not yet: editing inside a text after leaving it (Backspace from its end only).
  (Shift+Enter for a line break: done in M13b.)

Each milestone: `cargo test`, `cargo clippy` clean, then stop for manual testing.

## M13a — Calls in text: bold, italic, `#name[…]` (2026-11-09)

- Model: `Inline::Call { name, args, body }`, the body an inline list with the
  paragraph invariant (text, formula or call, text, …), so calls nest as in Typst.
  Bold and italic are the calls `strong` and `emph`. Path step `Step::Body` into a
  call's body (`[Index(1), Body, Index(0)]`); `Document::inline_at` finds the innermost
  inline list a path goes through (owner, list, index), and everything that took the
  inline index from `path[0]` uses it (`TextPos` has `owner`/`prefix`, `RowPos::base`
  is the path length to the formula). `Seq::Inlines(id)` takes a call's id for its
  body; `Selected::Formula` became `Selected::Inline { path }` (formula or call).
  File format 9.
- Typing (`markup_char`, `insert_inline`): `*`/`_` start `strong`/`emph` with the
  caret inside, and leave the innermost call when it is the same kind; after a letter
  or digit they are plain text (measured with Typst's parser: `a*b*c`, `snake_case`
  are text); `\*`, `\_`, `\]` turn the backslash into the char. `#name[`,
  `#name(args)[` and `#name` + space (not the whole paragraph, which stays a block
  call) make a call, also right after a letter (`word#footnote[…]`; fixed in M16b,
  measured: Typst reads it as a call); `]` and Esc leave the innermost call. A call
  left empty goes
  (on `*`/`]`/Esc at once; when the caret moves away, as its own undo step,
  `drop_left_call`, like empty titles).
- Backspace after a call selects it, then deletes it (as formulas); at the start of
  a body: an empty call goes, else ←. Enter inside calls splits them with the
  paragraph (`split_inlines`): the new paragraph starts with the rest of each call,
  the caret inside; a call with nothing after the caret is not continued.
- Rendering: `#name(args)[body]` in the editor (an empty body shows the transparent
  `x` of empty paragraphs, so the caret has a glyph); `;` after the call when the
  text after it starts with `.` or `(` (it would continue the expression). Export
  writes `*…*`/`_…_` when Typst reads them back the same (no letter or digit
  touching the delimiters, no space inside them, no empty text next to them, not
  inside the same markup), else the call (`markup_ok`; tests compare the pixels
  of both styles).
- Caret: `list_stops` recurses into bodies; the positions just before and after a
  call take the place of its first and last inner stop (correct across line wraps;
  the box only as a fallback).
- Not yet: Ctrl+B/Ctrl+I on a selection (no text selection yet); editing a call's
  arguments after typing them.
- Follow-up (user request): the call the caret is in (bold, italic, any `#name[…]`)
  is tinted and outlined like an environment: `Editor::call_focus` gives the
  innermost call, `caret::call_outline` one box per line from the stops inside it
  (a wrapped call gets a box on each line), drawn by `snapshot_call_outline`.

## M13b — Inline code, line breaks, `~`, dashes (2026-11-09)

- Inline code: `Inline::Raw(String)`, a non-text inline in the alternation (text,
  code, text). The caret goes inside it like text (`Target::Text`, its chars are
  positions; `Editor::raw_pos`), but none of the text rules apply: `raw_char` keeps
  every char as typed, `` ` `` or Esc leave it (empty code goes, also when the caret
  moves away: `drop_left_call` handles it), Backspace/Delete take chars, at its start
  ←; Enter leaves it first. From outside it behaves like a formula (Backspace after
  it selects it). Written `` `…` ``, or `#raw("…")` when it holds a backtick (or is
  empty, in the export); empty in the editor: the transparent `x`. Raw glyphs map to
  chars like text (measured: every char gets its own stop, evenly spaced). File
  format 10.
- Line break: a `\n` in the text, written `\ ` (a space rather than a newline, so it
  works in headings too). Typed `\` then space (as in Typst; `markup_char` turns the
  `\` into the break) or Shift+Enter (`Editor::line_break`, which in a formula
  inserts a formula line break). `collapses()` treats `\n` like a space for the
  no-double-spaces rules (a space after a break is not kept), and Backspace removing
  a char between a break and a space takes the space, never the break. A break that
  leaves an empty line (at the end, or before another break) is followed by the
  transparent `x` in the editor, mapped to the position after the break, so the
  caret shows at the start of the new line.
- `~` types a non-breaking space (U+00A0, written `~`); `--` → –, then `-` → —
  (stored as the chars, written as is). A `\` before `* _ ] ` ~ -` types that char
  literally. Typing normalises other whitespace to a space but keeps U+00A0.
- Not yet: showing non-breaking spaces (they look like spaces); `-?`, `...`.
- Follow-up (user request): inline code is outlined like calls and environments
  while the caret is in it; `Editor::call_focus` now gives the path prefix of the
  positions inside the innermost call (ending with `Body`) or inline code, which
  `caret::call_outline` matches.

## M15a — Labels (2026-11-09)

- Model: `BlockNode::label: String` (empty for none), used where
  `Block::labelable()` (headings, display formulas, environments). Serialized with
  `#[serde(flatten)]` on the kind, so a block without a label is written exactly as
  before; file format 11. `Seq::Label(block)` edits it like text; caret path
  `[Step::Label]` (`Target::Label`), its positions after the block's own (an
  environment's before the position just after it), so → at the end of a block goes
  onto the pill and ← from the next block comes back to it (`neighbour` now crosses
  blocks to the last *position* of the block before, not `end_of`).
- Editing (`label_char`, `leave_label`): only the chars Typst allows in a label
  (`is_label_char`: letters, digits, `- _ . :`); space, Enter, Esc or → at the end
  leave it (a new paragraph at the end of the document). Backspace/Delete take
  chars, ← at its start goes back into the block. In an environment's label the
  environment stays outlined (`env_focus`); `title_env` and the empty-title cleanup
  ignore the label path.
- Source: ` <label>` right after the block (`= Intro <sec>`, `$ x $ <eq>`,
  `#lemma[…] <l>`), in both styles; a test compiles a document referencing all three.
- Widget: the pill sits right of the page on the block's first line (a display
  formula's line): `caret::label_stops` puts one stop per char at a fixed advance
  (`PILL_CHAR_PT`) and the pill's text is drawn char by char at those places in a
  monospace font, so caret and text agree without measuring. Stops carry `label`;
  clicks and ↑/↓ reach them only right of the page, Home/End never.
  `snapshot_label_pills` draws every label, and a faint "+ label" for the caret's
  labelable block or the environment around it (`Editor::label_focus`).
- Not yet (M15b): references, the popover, duplicate/dangling labels.
- Follow-up (user request): the sample document shows every feature. Labels on the
  first heading, display formula and theorem; a `ket` macro in the header; a last
  section typed key by key through `Editor` (`sample::typed_section`): markup,
  inline code, dashes, `~`, a line break, formula calls, primes, text, a macro, an
  inline diagram, labelled `cases` and aligned formulas, a term list and a block
  call with an argument. Test `sample_has_every_feature` checks the export.
  (Typst 0.15 has `chevron.r`; `angle.r` is gone.)
- Follow-up (user request): `cases` by lines. Enter with the caret anywhere in a case
  of a call taking more arguments starts a new empty case after it
  (`Editor::case_around`, `new_case`; a call opened with `(` is still left with `)`),
  and ↑/↓ move to the case above/below (`vertical_target`; past the first/last case,
  line moves as before). The source keeps Typst's commas.

## M15b — References (2026-11-09)

- Model: `Inline::Ref(label)`, a non-text inline (alternation like a formula) with no
  caret position inside. File format 12.
- Typing: `@` (`start_ref`) inserts an empty reference and puts the caret after it in
  the "being typed" state (`word` == caret, `ref_before`); label chars typed then go
  into it (`ref_char`), anything else ends it first (`finish_ref`, also run by
  `finish_word`, so every other command ends it): a trailing `.`/`:` moves back into
  the text after it, and an empty one becomes a literal `@`. Backspace while typing
  takes its last char, then the reference. `complete_ref` sets the label from the
  completion list. A finished one is an atom: Backspace after it selects, then
  deletes. `\@` types a literal `@` (measured: Typst reads `a@b.c` as a reference, so
  there is no exception after letters).
- Source: `#sem-code[@label]` in the editor (the box carries the span), `@label` in the
  export, `#ref(<label>)` when the text after it would continue the label (`.5`).
- Caret: the texts around a reference end and start at its box.
- Rendering (blocks compile alone, so a reference has no target there): a labelled
  block's suffix gets `label_report`, metadata `<sem-labels>` giving for each label
  the counter a reference shows and its value there (`none` if it cannot be
  referenced: no numbering, defined twice, not a heading/equation/figure); the
  worker returns it (`Rendered::targets`, cached with the layout). `stale_blocks`
  builds the label table (defining block, count, report) and gives a block with
  references, after everything else, a stand-in per resolvable label (`ref_stub`):
  the counter set one below, then `#place(hide[<element> <label>])`, the element a
  heading of the same level, an equation, or a call of the same environment
  (`labels_in`). Typst then renders the reference with the header's supplements
  and numbering. Other references get a show rule in the prefix (`missing_ref`)
  showing a red `@label`. New targets trigger a re-submit. Measured: the stand-in
  render's text equals the whole document's, and stand-ins add no height
  (`references_through_stand_ins`); a forward reference settles in two rounds
  with every render compiling (`references_resolve_through_reports`).
- A label defined twice shows its pill in red.
- Completion: while a reference is typed, a list under the caret offers the labels
  containing what was typed, in document order, each with what it labels and its
  number from the reports ("equation 3", "lemma 1"). Drawn by the widget like the
  pills (no GTK popover: the editor keeps the focus and the input method), keys
  taken before the normal ones: ↑/↓ choose (back to the top when the typed label
  changes), Enter or Tab take it (`Editor::complete_ref`), a click on a row too;
  other keys type on and refine it.
- Navigation: a click on a reference (the node under the click, `BlockLayout::hit`,
  is a reference: `Document::ref_label`) follows it (`Editor::follow_ref`): the caret
  goes to the start of the labelled block (an environment's body) and the old caret
  is pushed on a stack; Alt+← (`go_back`) pops it (skipping blocks deleted since).
  The arrow keys still pass over a reference as an atom.
- The sample's last paragraph has references to a heading, two formulas and the
  theorem (its test checks that they point to labels of the document).
- Follow-up (user testing): references are followed with Ctrl+click (a plain click
  placing the caret was more useful; the status bar says so next to a reference),
  and every reference gets a light green background drawn by the widget behind its
  box (`snapshot_ref_marks`, `BlockNode::ref_ids`), so links stand out whatever the
  header's styling. Hyperlinks (`#link(…)[…]`) too, one box per line from the stops
  inside them (`BlockNode::links`, `caret::call_outline`).
- Not yet: a reference still being typed when the file is saved keeps a trailing
  `.`/`:` (the next key would have moved it); citations (M16).

## M16a — Figures with captions (2026-11-09)

- Model: `Block::Caption(inlines)`, the last block of a figure's body: a text block
  for the caret (`text_inlines`, new `own_inlines` for start/end positions), but not
  a paragraph (`inlines()` leaves it out, so it never joins or splits into a display
  formula). File format 13.
- `#figure ` (a block call) starts with an empty caption under the body paragraph;
  → from the body goes on into it. Enter in a caption leaves the figure (caret just
  after it); Backspace at its start: an empty caption goes, else ←. An "empty"
  figure (empty paragraph + empty caption) is removed by Backspace like an empty
  environment; unwrapping turns the caption into a paragraph. Status bar hint.
- Source: `#figure(caption: [Cap])[…]` (with other arguments: `args, caption: […]`),
  the caption written through `w.block`, so its spans map and the caret works in
  it; an empty caption shows the transparent `x` in the editor and is left out of
  the export. Labels and references work as for theorems (the stand-in is
  `#figure[]`, counter `figure.where(kind: image)`).
- The sample's chain diagram is now a captioned, labelled figure, referenced in the
  last paragraph.

## M16b — Footnotes numbered through the document (2026-11-09)

- Footnotes are the inline call `#footnote[…]` (M13a; typed right after a word,
  `word#footnote[`, since commit 49cb67e). `counter(footnote)` joins the tracked
  counters, and a paragraph holding a footnote (in other calls too, `has_footnote`)
  is counter-sensitive, so it starts from the previous block's count
  (`footnotes_are_numbered_through_the_chain`).
- Measured: in a block's page (height auto) Typst puts the footnote under the
  paragraph, after a separator line; the editor shows it there, its text mapped to
  the call's body, so it is edited in place. The export puts it at the foot of the
  page as usual.
- Caret: the position just after a footnote is right after its marker on the line
  it is called from (`FOOTNOTE_MARK_PT` past the text before it), not after the
  note's text below.
- The sample has a footnote in the markup paragraph.

## M16c — Bibliography and citations (2026-11-09)

- Model: `Block::Bibliography(source)`, BibTeX or Hayagriva YAML, behaving like a
  diagram block (source edited as a whole in the source dialog, two caret
  positions). "Insert Bibliography" (menu) puts one at the top level after the
  caret's top-level block, from a one-entry template, and opens the dialog.
  `bib_keys` reads the entry keys (BibTeX `@type{key,`, not `@string`/`@comment`/
  `@preamble`; YAML top-level `key:`); `Document::bibliography` is the first one.
  File format 14.
- Citations are references (`Inline::Ref`) whose label is a bibliography key and
  not a label of the document, as in Typst. The completion list offers the keys
  ("citation"); following one (Ctrl+click) goes to the bibliography.
- Source: `#bibliography(bytes(```bib … ```.text))` (`typst_bridge::bib`), the
  source verbatim in a raw block whose fence outgrows any backticks in it.
- Rendering (blocks compile alone): a citing block gets the citations of the
  blocks before it as hidden ones before it (`hidden_cites`) and the bibliography
  hidden after it (`hidden_bibliography`), so Typst numbers its citations as in the
  whole document; the bibliography block gets all the document's citations hidden
  before it, so it lists what is cited. Measured against a whole-document compile
  (`citations_through_hidden_ones`): same text, and the hidden parts take no space.
  A key that is neither a label nor in the bibliography shows a red `@key`.
- The sample ends with a bibliography of two MPS papers, cited in the last
  paragraph.
- Not yet: a bibliography from a file (needs project files); citation forms
  (`#cite(<key>, supplement: […])`, `form:`) other than `@key`.

## M17 — Opening `.typ` files (2026-11-10)

- `typst_bridge::import_typ` (`deserialize.rs`), the counterpart of the serializer:
  Typst's parser, no compile. "Open" accepts `.typ` files too; the document opens
  untitled ("Paper.typ (imported)") and is saved as a new `.sem` named after it.
  The `.typ` file is never written (one-way, as decided). An imported document
  counts as unsaved until it is saved as `.sem`, so closing it asks first (user
  request, 2026-11-10).
- Header: everything before the first content (`#set`, `#show`, `#let`, `#import`,
  comments). The editor's own preamble at the start of an exported file is dropped,
  and its CeTZ import too when the document has diagrams (the export adds it).
- Mapped: headings, paragraphs, display formulas, labels after them, bullet,
  numbered and term lists (nested), environments, figures with captions, diagrams
  and the bibliography in the editor's export form; in text, bold, italic, inline
  calls `#name(args)[…]`, links (a bare URL becomes `#link("url")[url]`), inline
  code, references, line breaks, `~` and dashes; in formulas, everything M12 knows
  (fractions, scripts, primes, calls, symbols by name or shorthand, text, `&`, `\`,
  roots, code `#…` and inline diagrams).
- A call standing alone is an environment when its body holds blocks, starts on a
  new line or has a title (`#lemma[Title][…]`), else a paragraph with an inline
  call; `#figure` always is one.
- Everything else stays raw Typst, verbatim and whole: code in the middle of the
  document (a code block, M10), a paragraph or formula holding anything the tree
  cannot represent (`#h(1em)`, comments, `mat(…; …)`, named arguments in math, a
  label after a paragraph), an enumeration item with an explicit number.
- Measured: bars in formulas. Spaces around `|` show; the tree's bar is written
  `bar.v`, which renders like `a | b`. A bar touching its neighbours (`|x|`) is a
  different rendering the tree cannot write, so such a formula stays raw (wishlist).
  Superseded by math spaces (below, 2026-11-10).
  `dif` and other names only in the math module are kept as code `#math.dif`.
- Tests: importing then exporting renders the same pages (a paper with every
  feature, formulas, raw fallbacks, lone calls); the sample's export opens as the
  same document (exports to the same Typst, no new raw blocks).
- Found by the round trip, fixed in its own commit (da29653): a `;` right after an
  inline call ended the call's expression and vanished from the export; it is now
  doubled.

## M18 — Title block (2026-11-10)

- Model: `Block::Title(source)`, a block like the bibliography (two caret
  positions, edited in a dialog), its source the fields as lines (`TitleFields`:
  title, date, then one author per line). "Document Title…" (main menu) puts one at
  the start of the document, or moves the caret onto the one there is; there is at
  most one. File format 15.
- The dialog: Title (Typst markup), Authors (comma separated), Date (`YYYY-MM-DD`,
  optional, flagged when invalid) as entry rows; every change goes to the model at
  once (one undo step, as for source edits); Enter or Done closes it. Double-click
  or Enter on the block opens it, as for diagrams.
- Source: the export sets the metadata first, `#set document(title: […], author:
  (…), date: datetime(…))` (before the header, so a template there can read it), and
  the block shows `#title()` (Typst's title element, restyled with `show title` in
  the header) and an author line, `#context document.author.join(", ")`. In the
  editor the block is rendered alone with its own `set document`; an empty one shows
  a grey "Title". The PDF gets the title, authors and date (`title_block` test).
- Import: the export's form (the metadata line and the title and author line as the
  first blocks) opens as a title block again; other `set document` rules stay in the
  header.
- The sample starts with a title block.

## M17 follow-ups: the user's paper (2026-11-10)

- `examples/import_report.rs`: imports a `.typ` file and lists the blocks that came
  out raw; `--compare` renders the original and the export of the import and names
  the differing pages (saving them as PNGs), `--against FILE` compares two sources.
  Files go to `target/import-check/` (the SDK sandbox sees only the project), which is
  ignored by git; the paper itself is never committed.
- A `#set` rule inside a paragraph (after its `#`) no longer splits the paragraph
  there (the `#` was left behind and the export did not compile); the paragraph
  stays raw as a whole, the rule applying to its rest as in Typst.
- A block call on lines of its own right under a line of text (`#theorem[…]` with
  no blank line before it) ends the paragraph, as it does in Typst: the theorem is
  an environment and the text before it a paragraph (they were raw together). A
  labelled call the tree cannot hold (`#lemma(statement()) <lem:a>`) is a raw block
  of its own, so the text before it stays editable. User report: theorems came in as
  raw Typst.
- An escaped bracket in a formula (`\{h, overline(h)\}`) does not pair up and
  stretch in Typst, the tree's brackets do: it is imported as the named symbol in
  code (`#sym.brace.l`).
- Spaces next to text in a formula show (measured, `"Rep"(A)` vs `"Rep" (A)`).
  The export now writes text before an opening bracket tight (`"Ker"(A)`, the usual
  operator form), in the editor too; a formula whose text has other spacing than
  the export writes (a space before a bracket, text touching the atom before it)
  stays raw.
- A list starts on the line after the block before it in the export, as in bodies:
  a blank line between a paragraph and its list adds space in Typst (measured).
  With these, the user's 55-page paper imports and exports to identical pages;
  58 of 964 blocks stay raw (figures drawn by a function, statements produced by a
  function, `lr(size: …)` and other named arguments in math).
- A raw block's preview failed when its source referenced a label in another block
  (a raw figure whose caption cites a theorem: a red dot instead of the figure).
  References in raw Typst are now found (`refs_in_source`) and get a stand-in for
  the target, or the red `@label`, like references in the tree.

## Problems: Next Problem (2026-11-10)

User request: an option to look for problems. Done:
- `label_problems` (jobs.rs) finds references to missing labels, references to
  labels on raw Typst (shown red), and labels defined twice, per top-level block.
  Labels and references in raw Typst come from its syntax (`labels_in_source`:
  only labels in markup; `refs_in_source`); BibTeX keys in the header or raw
  blocks count as citations.
- The widget adds blocks whose compile failed, one entry per block in document
  order; `go_to_problem` moves the caret there (wrapping) and returns "k of n" and
  the message.
- The app: actions next-problem (F8) and previous-problem (Shift+F8), menu entry
  "Next Problem"; a toast names the problem and replaces the previous one (markup
  off, labels have `<…>`).
- On the user's paper: two references remain, to labels made with `label("…")`
  in code; the rest were false alarms now fixed (a label passed to `restate`,
  citations to the bibliography written in the document).

### Figures whose body is code

User report: references to the paper's figures said "points to raw Typst". Its
figures write the body as the first argument, `#figure(dia(…), caption: […])` or
`#figure({ … }, caption: […])`, so the importer (which only took
`#figure(…)[body]`) kept them raw. `code_figure` (deserialize.rs) now makes them
figures: a body in brackets becomes blocks, code becomes a raw Typst block
`#dia(…)` / `#{ … }` inside the figure, with the caption and label as usual. The
export writes `#figure(caption: […])[…]`, which renders the same (test
`a_figure_with_code_for_its_body_is_a_figure`). On the paper all four figures
import as figures and their references are numbered.

### Numbered references to labels on raw Typst

User report: `@eq:B_A_relation` still "pointed to raw Typst": its equation
stays raw (bars touching neighbours, `z_(|partial v|)`), and labels in raw
blocks had no stand-in. Now `labels_in` (jobs.rs) lists the markup labels of
non-code raw blocks with an empty element; the block's label report (all raw
blocks are counter-sensitive) gives the counter, and `stand_in` (counters.rs)
turns it into the stand-in: a heading of that depth, a block equation, or a
figure of kind image/table/raw. Figures of other kinds (theorems made with
figure) have unknown supplements and stay red. `label_problems` takes the
reported targets, so only those are still listed. On the paper: no problems left.

## Spaces in formulas (2026-11-10)

- Why: Typst (`typst-library` math `ir/process.rs`, `spacing()`) keeps a written
  space only next to a "spaced" item: a fence (`|`, `‖`, `⦀`), text (`"if"`) or a
  box, and not at a group's edges, after a line break, or where a relation or binary
  operator already puts its own spacing. So `|x|` ≠ `bar.v x bar.v` and
  `"Rep" (A)` ≠ `"Rep"(A)`; such formulas stayed raw (16 blocks of the user's paper).
- Tree: `MathNode::space`, a space before the node. Created only where it shows
  (`spaced_gap`: next to a bar or text, not after a line break); a deletion can make
  one dormant (kept, not written), as a space in Typst source would be. `.sem`
  format 16 writes a spaced node as `{"space": true, "kind": …}`, others as before;
  files before 16 get the spaces their export wrote (`space_bars_and_text`).
- Import records spaces at shown gaps; export writes them there and nothing else
  next to bars/text (elsewhere as before). A bar is written `|`, or `\|` where a
  shorthand would form (`||`, `[|`, `|]`, `|->`, `|=>`); `\|` renders the same.
- Typing: `|` gives a tight bar; space puts a space into the gap at the caret where
  it shows, else is held for the next atom typed (`held_space`, as in prose); it
  also keeps `|` `-` from combining into ⊢. Backspace/Delete take a shown space
  first; Backspace right after a held space forgets it.
- Code atoms (`#…`) are boxes too; see "Code next to a space" (2026-11-13).
- Check: paper import 46 → 30 raw blocks, 55 pages render identical.

## Restated blocks (2026-11-10)

- User report: `#restate(<lem:main_mps>)` showed "array is empty" in the editor. Every
  block compiles on its own, and the user's `restate` starts with
  `query(lbl).first()`, so the lemma must be in that compile.
- `jobs::stale_blocks`: for labels a raw block names as values
  (`label_values_in_source`: labels outside markup), each defined once by another
  rendered block, the prefix starts with that block's counters at its start (its
  last render's `start`, kept in `Submitted`), then `#place(hide[…])` around its
  readable source; the block's own counter prefix follows and sets them back, so
  its end state is unaffected. `restate` then gets the lemma's number and the
  equation count at it (its offset for the restated equation numbers). The copy's
  references and citations are handled as the block's own, except labels it defines.
- Before the lemma has rendered once, no copy (the error shows until it has).
- Test: `restated_blocks_find_what_they_name` (fails with "array is empty" without
  the copy). On the user's paper all three restates compile.
- Found on the way, own commit: references in a block call's source arguments
  (`#proof(title: [Proof of @lem:x])`) were not seen, so they got no stand-in and
  the block failed ("label does not exist"). `refs_in` now reads `Env.args` too.
  With both, every block of the user's paper compiles in the editor.

## Hand-written bibliographies (2026-11-10)

- User report: citations showed `@key` and the References section printed nothing.
  The paper's bibliography is `#bibliography(bytes(```…```.text), title: "References",
  style: "ieee")`, which the import keeps raw (the tree's bibliography has no
  arguments), and the editor only set up citations for its own bibliography block.
- Now a raw block that is one `#bibliography(…)` call (`bib::raw_bibliography_call`,
  by the parser) is the document's bibliography too: citing blocks get that call
  hidden after them (so its style numbers them), the block gets all citations hidden
  before it (so it lists them). Keys of its embedded BibTeX: `embedded_bib_keys`.
  `Document::bibliography`/`bib_keys` see it as well (citation completion, following
  a citation). No tree or format change.
- Checked on the user's paper: `[1]` in the text, the References list in IEEE style.
- Known limit (2026-11-10, left for later): a restated lemma with a footnote shows
  the note twice in the editor: the hidden copy's note is printed too. Measured:
  Typst prints the note of a footnote inside `hide`, `place` or a zero-height clipped
  box even with `show footnote: none` / `show footnote.entry: none` in or around it
  (a document-level `show footnote: none` works only on unhidden content). Options:
  drop footnotes from the copy's source (preview then shows none), or find how Typst
  collects notes from hidden content.

## Greedy lines while typing (2026-11-10)

- User report: typing in the middle of a paragraph was disorienting, the caret and
  the text around it jumping as Typst re-typeset the paragraph. The paper sets
  `par(justify: true)`, so Typst breaks lines "optimized" (Knuth–Plass), which may
  re-decide every break of the paragraph at each keystroke.
- Word processors and LyX break lines greedily on screen; the editor now does that
  for the top-level block being typed in (`jobs::GREEDY_LINES`,
  `#set par(linebreaks: "simple")` in its prefix, paragraphs/environments/items/
  captions). It starts at the first edit in the block (not on entering it, so moving
  the caret reflows nothing: `track_editing` compares the caret's block with its
  state at the last render request) and ends when the caret leaves it, which renders
  it again with the document's own breaking. Export unaffected.
- Measured: greedy breaking leaves the lines above an edit unchanged (test
  `greedy_lines_above_an_edit_stay`). On a sample, optimized breaking did not move
  the top quarter either over 34 keystrokes, so the jumps were probably words
  bouncing between the current and the next line. The user found it "helps quite a
  bit". Not addressed: justification re-spreading the current line (sideways shifts).

## Options of math calls, step (a) (2026-11-11)

- `MathKind::Call` has `opts`: its named arguments as Typst source, joined with
  `, ` (`size: #55%`), empty if none. The export writes them first,
  `abs(size: #2em, x/2)` for `abs(x/2, size: #2em)`, which Typst reads the same.
  The import keeps them instead of leaving the formula raw; spread arguments
  (`..x`) still stay raw.
- `.sem` format 17: `opts` on a call, absent when empty (older files load as is).
- Test `math_calls_keep_named_arguments` (`lr`, `abs`, `op` with `limits`).
- Paper: 30 → 25 raw blocks (all five `lr(size: …)` formulas), 55 pages identical.
- Not yet: the options are not shown or editable in the editor (step b).

## Options of math calls, step (b) (2026-11-11)

- Options are stored as code, as typed in the tab (`size: 55%`); the export
  writes each value after a `#` (`size: #55%`), in parentheses when `#` cannot
  take it as it is (`#(80% + 2pt)`, by `Expr::hash`), strings as they are. The
  import keeps a value after `#` or a string; other named arguments (math
  content, `attach(x, t: y)`) leave the formula raw, as before.
- `.sem` format 18; files of format 17 (options as written in math) are
  converted on load (`typst_bridge::code_opts`).
- Which calls have a tab: `model::OPTION_CALLS`, Typst's math functions (and
  callable symbols, the accents) with named parameters that are not content,
  or any call with options already (`MathKind::tab`). Test
  `option_calls_match_the_library` keeps the list in step with Typst.
- Cursor: `Step::Opts` after a call's `Index(k)`, `offset` counts chars
  (`Target::Opts`). Not among `Document::positions` (user request: ←/→ skip
  the tab): `vertical_target` gives it for ↑ from an argument of the call
  when nothing structural is above (numerator, earlier case of `cases`); the
  widget picks the char nearest the caret's x. In the tab ←/→ stay in it,
  Home/End go to its ends (`tab_edge`), ↓ (`tab_exit`) into the first
  argument. Edits go through `Seq::Opts(node)` (undo works).
- Typing: every char kept (spaces too); Enter or Esc → into the first
  argument; Backspace at the tab's start and Delete at its end do nothing.
  In an `lr` argument `(` is a bracket (elsewhere `(` in an empty argument
  opens its group), so `lr` + space then `(x)` types the brackets to scale.
- Widget: tab stops (`Stop::tab`) keep the formula's baseline and sit above
  the call's box (`caret::tab_rect`); line moves, Home/End, clicks on the page
  and the call outline skip them; a click on the shown tab (`caret::tab_hit`)
  reaches them. While the caret is in a call with a tab (`Editor::tab_focus`),
  the call is outlined (under the render) and the tab drawn over the render,
  "+ options" when empty. Test `call_option_tabs`, `math_call_options`.

## One contour around a multi-line call (2026-11-11)

- User report: the outline of a call over several lines (`#audit[…]`, `#cl[…]`)
  was one box per line, and the boxes overlapped. `caret::contour` turns the
  lines' boxes (padded by `CALL_PAD_PT`) into polygons: consecutive lines that
  overlap horizontally meet halfway between them and are traced as one shape
  (down the right edges, up the left ones), straight-edge points dropped; a line
  that does not overlap the one before starts a shape of its own. Drawn as a
  `gsk::Path` (fill and 1 px stroke) for inline calls and math calls with a tab.
  Test `outline_contour`.

## Unwrapping inline calls (2026-11-11)

- User request: Backspace at the start of a call's body (`#cl[…]`, `#audit[…]`,
  bold) unwraps it, as for environments. First press: `Selected::UnwrapCall`
  (hint "Backspace → remove the call, keep its text", outline drawn stronger,
  `Editor::call_marked`; Esc cancels). Second press: `unwrap_inline` deletes the
  call and the text after it, moves the body's inlines in (ids kept), the text
  before takes the body's first text and the body's last text takes the text
  after (`joined_tail`, so spaces collapse as Typst would); the caret stays where
  the body started. An empty call still goes at once. Test
  `inline_calls_unwrap`; `backspace_around_inline_calls` updated (it used to
  move left).

## Selection, step 3a: selecting (2026-11-11)

- `Editor::anchor` (the other end; the caret is the head): kept by Shift + a
  move (`anchor()` before, `set_anchor` after; the widget's `run_selecting`),
  Shift+click, a drag (`GestureDrag`: anchor at the press point, head at the
  pointer); cleared by any other move, edit, `set_caret`, Esc.
- `Selection::normalized` reworked: same container → `Text`/`Row`; a common
  math row → `Row` (branches whole); two children of one structure → the
  structure; else the innermost common inline list (`SelRange::Inlines` now
  has `owner` and `prefix`, so it works in call bodies and titles); different
  blocks → whole top-level `Blocks`.
- `delete_range`: chars (`Text`, also labels and options; a raw block's whole
  text removes the block), nodes (through `Selected::Nodes`), inlines (texts at
  both ends joined with `joined_tail`), blocks (an emptied document gets an
  empty paragraph). Backspace/Delete/typing/Enter go through it; typing over
  a math range keeps it as `Selected::Nodes` so `/`, `^`, `_` wrap it.
- Drawing: `selection_bounds` gives both ends as positions; every stop between
  them in the block's stop order, grouped by line, gives one band per line;
  `Blocks` shade whole block views. Test `range_selections`, selection tests.

## Selection, step 3b: copy and cut (2026-11-11)

- `Editor::selection_fragment` → `model::Fragment`: `Blocks`, `Inlines` (texts cut
  at the range's ends, formulas/calls whole), `Math` (row nodes), `Plain` (chars
  of a label or options); a raw block's whole text copies the block.
- `typst_bridge::fragment_source`: Typst as the export writes it: inlines as a
  paragraph's text (markup escaped, `a \*`), math nodes as a formula's content
  without `$`, blocks joined as in the export. Test `fragments_as_typst`.
- Widget: Ctrl+C puts it on the clipboard as text; Ctrl+X too, then deletes
  the range. A status-bar hint names the keys while a range is selected.

## Selection, step 3c: paste (2026-11-11)

- Ctrl+V reads the clipboard's text (async) and `typst_bridge::parse_fragment`
  reads it as Typst: in a formula (`Editor::in_math`) as `$…$` → math nodes, or,
  if that is not one formula, plain text typed in; elsewhere imported after a
  dummy first paragraph (so leading `#set`/`#let` are content, not a header):
  one unlabelled paragraph → inlines, else blocks.
- `Editor::paste` (`editor/paste.rs`): fresh ids for the fragment
  (`Document::adopt`); a selected range is deleted first and the paste joins it
  in one undo step (`Last::Chained`). Math nodes go into the caret's row (in
  text: as an inline formula); inlines are spliced into the caret's text, the
  texts at both seams joined (`joined_tail`), also in a call's body; blocks
  replace an empty paragraph, go before/after the caret's block at its start/
  end, or the paragraph is split (Enter) and they go between; at an
  environment's edge before/after it. Tests `pasting`, `pasted_typst`.

## Labels only by click (2026-11-12)

- User: the arrows should not enter a label; only a click on its pill does.
  `Document::positions` has no label positions, so ←/→ pass it by, and the
  widget's line moves and Home/End never pick label stops (`reachable`,
  `line_edge`); a click finds the pill first (`caret::label_hit`).
- In the label ←/→ and Home/End stay in its text (`Editor::field_pos`, shared
  with the options tab); space, Enter, Esc, ↑ and ↓ leave it to the end of its
  block, an environment's to just after it (`Editor::label_exit`).
- `Editor::enter_label` does what the click does (the sample and tests use it).
- Open: a keyboard way into the label (and the options tab), proposed to the
  user, not yet chosen.

## Equation labels at the number's height (2026-11-12)

- User: a display formula's label pill should sit where its number is, in the
  middle of the formula, not on its first baseline. Typst centres the number
  (`number-align: end + horizon`), so the pill is centred on the extent of the
  formula's caret stops (`block_stops`, `label_stops(.., mid, ..)`). The stops
  keep the first line's baseline for grouping into lines; the pill's text is
  drawn from its bottom edge. Test in `label_pills` (a fraction: above the
  baseline).

## Footnote text after its paragraph for ←/→ (2026-11-12)

- User: → on a footnote's marker should not enter the footnote; its text is
  reached where it is shown, after the paragraph. `Document::positions` puts
  the positions of a footnote's text after the rest of its paragraph (several
  footnotes in reading order, a footnote's own footnotes after it), so the
  marker is one step for ←/→. ↑/↓ already go by the layout. Test
  `footnotes_after_their_paragraph`; `stops_in_inline_calls` compares a
  footnote paragraph's stops with the positions as sets.

## Outline pane (2026-11-12)

- User: view and navigate the table of contents even if the document has no
  `#outline()`; a side pane is fine. `window/outline.rs`: a list of the
  top-level headings (indented by level, level 1 bold; formulas as their
  source, footnotes left out, `heading_titles`) in the sidebar of an
  `adw::OverlaySplitView`, left of the editor, max 260 px; shown by default,
  a header-bar toggle hides it. Updated on `caret-moved` (rows rebuilt only
  when the headings changed); the heading the caret is under is selected.
  Activating a row: `SemEditor::go_to_block`, focus back to the editor.
- Not yet: headings inside environments, remembering whether the pane is
  shown. (Heading numbers: see the next section.)

## Outline entries rendered by Typst (2026-11-12)

- User: headings hold formulas, so the outline should be Typst's rendering,
  with the heading numbers. Each heading, once its own render arrives, is
  compiled again as an outline entry: its last job (header, counters before
  it, snippet) with `typst_bridge::outline_entry_rule` in the prefix, a show
  rule drawing the number (`counter(heading).display(it.numbering)`) and the
  title at 11pt, level 1 bold, indented 1em per level, in the widget's text
  colour (dark mode). 180pt wide at 1.25 px/pt; cropped from the page margin
  to the ink. The jobs go to the same worker with the heading's id plus
  `OUTLINE_BIT`; `SemEditor::outline_entry`, signal `outline-rendered`.
- The pane shows the render (`Render`, a texture at its logical size), the
  plain title until it arrives or if it fails. Test `outline_entries`
  (numbers from the counters before, size independent of the header's
  heading styles).
- Not yet: headings inside environments; a theme switch re-renders entries
  only when their heading is compiled again.

## Jumps land a third down the view (2026-11-12)

- User: a destination (outline entry, any link) should be placed in the top
  third of the view if possible. `reveal_third`: set by `after_jump` (used by
  `go_to_block`: outline and problems; Ctrl+click on a reference) and by
  Alt+← back; the next `reveal_caret` puts the caret's top a third down the
  view, clamped to the scroll range (near the document's ends it cannot), and
  waits for the target block's render like the plain reveal. Other moves keep
  the minimal scroll.

## Outline rows: centred, even spacing (2026-11-12)

- User: the selected entry's text sat high in its highlight; a wrapped entry
  had no gap to the next one; the gaps were as high as a line. The sidebar's
  rows have a minimum height, so one-line entries got more space than wrapped
  ones, and the render was drawn at the top. Now rows are as high as their
  entry plus 4px above and below (CSS `.outline-pane > row`), the render is
  centred in its row, and entries are cropped to their line box, not to their
  ink: the entry rule sets the text's edges to ascender/descender (every line
  the same height, descenders or not; wrapped lines with 0.25em leading), the
  crop keeps the page's content box widened to any ink outside it.

## Outline entries: hanging indent (2026-11-12)

- User: a wrapped title's second line should start under the title, not
  under the number. `outline_entry_rule` puts a numbered heading in a grid,
  number | title (0.5em apart, as `outline()` indents its entries); the test
  `outline_entries` checks a wrapped entry's lower half starts right of its
  number.

## Projects, step 1: files in the document and the compiler (2026-11-12)

- `model::Files`: the project's files by path (`images/chain.png`), shared
  until changed, each with its hash computed once (`Hash` uses those, so
  cache keys do not rehash bytes); `free_path` for a new file's name.
  `Document::files` (not in `document.json`; the zip stores them, step 2).
- `Around::files`: every block's job carries the document's files
  (`stale_blocks_editing(.., files, ..)`, the preview too); the worker hands
  them to the world before compiling, and `BridgeWorld` serves project paths
  from them (bytes, or a `Source` for `.typ`). `compile_pdf(source, files)`.
  Test `project_files` (an SVG by its path).

## Projects, step 2: the zipped `.sem` (2026-11-12)

- `file::to_zip` / `from_zip` (crate `zip`, deflate only): `document.json`
  (the former JSON, format version restarted at 1: no files of the old
  format exist, user), `main.typ` (written on save, skipped on load), every
  other entry a project file. Open and save in the window use them; unsaved
  changes count the files too. The old migrations (spaces before format 16,
  math-form options of format 17) and their helpers are gone. Test
  `projects_are_zips`.

## Projects, step 3: picture blocks with options (2026-11-12)

- `Block::Image { path, opts }` (`Document::image`): a project file's path
  and the other arguments as Typst code. Written `#image("…", opts)`; a
  figure whose body is one picture is written in code form,
  `#figure(image("…", width: 2cm), caption: […])`, and read back from it
  (`Parser::picture` in `code_figure`; anything but a path and named
  arguments stays raw). Bare `#image` outside a figure stays raw (figures
  only, user).
- For the caret it is like a diagram (`Target::Raw` with its path: before
  and after it, Backspace selects it, Enter makes a paragraph), but has no
  source to edit (`source_target` skips it) and an options tab:
  `Step::Opts` on the block, `Target::Opts { id }` and `Seq::Opts(id)` now
  take a math node or a block. `tab_focus` is the empty path beside a
  picture, so the widget outlines it and draws its tab (stops above its
  box, `content_stops`); ↑ beside it enters the tab, ↓/Esc/Enter leave to
  just after it (`tab_exit`). Tests `figures_of_pictures`,
  `picture_options`, `picture_stops`.

## Projects, step 4: Insert Picture and the sample's picture (2026-11-12)

- Menu "Insert Picture…": a file dialog for pictures Typst reads (png, jpg,
  gif, svg, webp, pdf); `Editor::insert_picture(name, bytes)` stores the
  file as `images/<name>` (`Files::free_path`: `a-2.png` if taken) and puts
  a figure of it after the caret's block (in place of an empty paragraph;
  from a caption, after its figure), the caret in its empty caption. The
  file stays in the document on undo; the zip keeps only the files
  `main.typ` names (`to_zip`).
- The sample's typed section inserts `data/sample/correlations.png` (a
  12-site correlation matrix, generated), types its caption and label and
  `width: 40%` in its options tab. Tests `inserting_pictures`,
  `sample_has_every_feature`, `projects_are_zips`.
- Seen on the way, not fixed: `insert_block` (diagram, raw Typst) from a
  caption probably puts the block inside the figure after its caption.

## Projects, step 5: Export Typst Project (2026-11-12)

- "Export Typst Project…" asks for a folder (the portal grants it) and
  writes `file::typst_project`: `main.typ` and the files it names, their
  folders made as needed; an existing `main.typ` there is replaced only
  after asking. The zip writes the same set.
- `file::safe_path`: project paths are relative without `..`; `from_zip`
  rejects other entries, the export skips them (a crafted `.sem` cannot
  write outside the folder).

## Projects, step 6: the bibliography as `refs.bib` (2026-11-12)

- The exported Typst (`main.typ`, Style::Readable) writes
  `#bibliography("refs.bib")`; `file::typst_project` adds `refs.bib` with
  the block's source, so the zip, Export Typst Project and Export PDF
  (`project_files`) all have it. The editor's own renders still embed the
  source (no file needed per block), and the block keeps the source, so the
  source dialog, undo and the citation keys work as before (moving the
  source into `Document::files` would need undoable file ops; not needed
  for what the user sees).
- `import_project(source, files)` (`import_typ` without files): a
  `#bibliography("x.bib")` whose file the project has becomes the
  bibliography block with that source; without it, raw as before. The
  exported project opens as the same document again. Test
  `bibliography_files`.

## Projects, step 7: importing a `.typ` with its files (2026-11-12)

- `typst_bridge::local_files(source)`: the string literals that are
  relative paths of document files (pictures, `.bib`, `.typ`, data; no
  URLs, packages or `..`). Opening a `.typ` reads them from beside it; what
  cannot be read (the Flatpak grants only the chosen file) is read from a
  folder the user picks in a folder dialog (opened at the file's folder),
  which grants access to it. Files still missing are reported in a toast.
  `import_project` gets them, so pictures show, a `#bibliography("x.bib")`
  becomes the bibliography block, and the files go into the project.
- Not yet: files named by an included `.typ` (only the main file is
  scanned); paths built in code (`"figs/" + name`).
- Test `local_files_of_a_source`.

## Screenshots without a window on the screen (2026-11-12)

- `SEM_SAMPLE=1` opens the sample at start; `SEM_SCREENSHOT_DELAY` (ms) sets
  how long `SEM_SCREENSHOT` waits. Run on GTK's Broadway backend inside the
  SDK, with a private session bus (the sandbox may not own the app's name),
  nothing shows on the user's display:

  ```sh
  build-aux/sdk dbus-run-session -- sh -c 'gtk4-broadwayd :9 & B=$!; sleep 1;
    GDK_BACKEND=broadway BROADWAY_DISPLAY=:9 SEM_SAMPLE=1 SEM_SCREENSHOT_DELAY=10000 \
    SEM_SCREENSHOT=docs/screenshots/editor.png target/sdk/debug/semantic-editor; kill $B'
  ```
- Use a display number not used before (a stale one gave "nothing
  rendered"). The Broadway screen is 1024×768. A second shot after jumping
  to a label failed on and off; not pursued.

## History, step A: a git repository in the `.sem` (2026-11-12)

- `history.rs` (crate `git2` with a bundled libgit2, `tempfile`): a bare
  repository in a temporary folder (deleted with the window's `History`).
  `commit(files, author, message)` builds the tree from the files' paths and
  commits on `main` unless the tree is the last one's; `entries()` writes
  the repository for the zip: one pack of every version and tag
  (`PackBuilder`), `packed-refs`, `HEAD`, a non-bare config;
  `from_entries` rebuilds it. Versions, tags (annotated: the message keeps
  the typed name, the ref a safe form), the files of a version: for the
  next steps. Test `versions_and_tags`, `tag_ref_names`.
- `file::snapshot(doc)`: `document.json` and the Typst project, what a
  version holds; `to_zip(doc, history)` adds the history under `.git/`,
  `from_zip` returns the document and its history (a new one without).
- The window keeps the history of its document; every save commits first
  ("Saved", author `glib::real_name`, else the user name; libgit2 needs an
  email, so the placeholder `none`). A new, imported or sample document
  starts an empty one. If the temporary folder cannot be made, saving
  still works, without versions.

## History, step B: the Versions tab and named versions (2026-11-12)

- The sidebar has two pages, Outline and Versions (`adw::ViewStack` with an
  `InlineViewSwitcher` on top). `window/versions.rs`: "Name Current
  Version…" (`win.name-version`: saves, then tags the saved version), the
  named versions (tags, with day and time), then every save grouped by day
  (`adw::ExpanderRow`s, "Today", "Yesterday", "3 November 2026"; the newest
  day open), each save with its time and tags. Clicking a version offers to
  name it (step C opens it instead). Naming saves the file, so the tag is
  kept; the list is refreshed on load, save and naming.
- A name is asked in a dialog with an entry; any text, kept as typed in the
  annotated tag (see step A).

## History, step C: old versions read-only, and restoring them (2026-11-12)

- `Editor::set_read_only`: `commit`, undo/redo and `insert_picture` refuse
  every change; moving, selecting and copying work (`SemEditor::
  set_read_only`, kept across `set_document`). Test `read_only`.
- Clicking a version in the Versions tab opens it in a new window
  (`window/old_version.rs`): `History::files_at` → `file::from_snapshot`,
  read-only, the editing actions (save, open, insert, header) disabled, no
  Versions switcher, title "<name> — Today, 14:03 (read-only)", an
  `adw::Banner` "The version of …, read-only" with "Restore this Version".
- Restore: unsaved changes of the current document are saved first (a
  version), then the old content replaces the document and is saved as a
  new version, "Restored the version of …" (`commit_message`); the
  read-only window closes. Undo cannot take a restore back (the document is
  replaced), but every earlier version stays in the history.
- "Name Current Version…" in an old version's window names that version.

## History, step D: scratch files (2026-11-12)

- `model::is_scratch` / `SCRATCH_DIR`: files under `scratch/`. The zip
  keeps them all; `typst_project` (versions, Typst export) leaves them out;
  `project_files` (PDF) adds the ones `main.typ` names, so a draft PDF shows
  draft figures. Restoring an old version keeps the current scratch files.
- `Op::SetFile` puts or removes a project file (inverse: what was there), so
  file changes are undoable: Insert Picture's file goes with undo now.
  `insert_picture(name, bytes, scratch)`; `keep_file(path)` moves a scratch
  file to `images/` and repoints its pictures (an image block's path is now
  a text sequence for ops), one undo step; `delete_file` only for unused
  files (`file_used`: a picture shows it, or raw Typst, a diagram or the
  header names it). Tests `scratch_files`, `inserting_pictures`.
- UI: "Insert Scratch Picture…"; the Versions page ends with Scratch Files
  (hidden when there are none): name, "In the document" or not, Keep, and
  Delete when unused. Refreshed after every command (rebuilt only when it
  changed).

## A formula in text wrapped onto the next line (2026-11-12)

- User: in a formula in text that Typst wraps onto the next line, the caret
  spanned both lines, and ↑ did not go to the line above. Every stop of the
  formula had the height of its whole box (both lines) and one baseline.
  `row_stops`: without `\\` in the row, a node starting left of the one
  before it starts a new line; each line gets the height of its nodes and
  its own baseline (`line_near`), the stops of nested rows too. Just
  before and after the formula, the text's stops sit at its first and last
  stop (after it: where it ends on the last line). Test
  `wrapped_inline_formula`.

## Code next to a space in a formula (2026-11-13)

- User: in the PEPS definition, `$e = #sym.brace.l h, overline(h) #sym.brace.r$`
  showed as `{ h, h̄ }` in the editor, tight `{h, h̄}` in Typst. The editor lays
  code out as a `sem-code` box, and the edit style wrote a space on both
  sides; a space next to a box shows (measured). The export was right.
- Edit style: no space next to a code atom (only after `\`, which would
  escape the `#`), and the box ends with `;` (`#sem-code($#…$);`), so a
  following `(`, `[` or `.` is not read as part of it. Test
  `code_in_formula_renders_tight` (fails without the fix).

## Footnote text after its theorem for ←/→ (2026-11-13)

- User: in a theorem, ←/→ reached a footnote's text right after its
  paragraph, before a later display equation, though it is shown under the
  whole theorem (↑/↓ were right). `Document::positions` now collects the
  footnotes of an environment's or list item's blocks and puts them after the
  whole block, after its end edge. Test `footnotes_after_their_theorem`.

## Calls of the header's functions in formulas get their whole box (2026-11-13)

- User: with `#let im(x) = $upright("im")(#x)$` in the header, after `im(x)`
  the caret showed before the `)`; a display formula holding only it could
  not be navigated. What the function draws (name, parentheses) carries the
  header's spans, so the call's box covered only `x`. `im` is a predefined
  operator in Typst, so it can only be called as the header's function.
- Edit style: a call of anything that is not a Typst math function or
  callable symbol is written `class("normal", #sem-code() im(x) #sem-code())`:
  the empty boxes carry the call's span on both sides; `class` keeps it one
  piece for scripts and in the formula's style (a box around the call made a
  display fraction in its argument inline-sized; measured). Export unchanged.
  Test `calls_of_header_functions_cover_their_drawing`.

## R1 — Blocks keep their ids across save and load (2026-11-13)

- First step of the remarks (spec: Remarks, chat, suggestions, comparing
  versions). `document.json` now writes each block's `id` (nested blocks in
  environments and lists too); other nodes' ids are still not written.
  `Document::restore_ids` (used by `file::from_json`) keeps the saved ids, a
  repeated one only the first time, and gives every other node, and blocks of
  older files without ids, a fresh id above them. Old files load as before;
  they get ids on the next save. Nothing visible changes. Tests
  `json_roundtrip_keeps_block_ids`, `nested_block_ids_are_kept`.
- Not checked yet: which edits replace a block by a new one (new id), e.g.
  changing its kind; to look at with the anchors (R2).

## R2a — Remarks in the model, following edits (2026-11-13)

- `model::remarks`: `Remark { id, anchor, text, author, date, resolved }`,
  `Anchor::Block(id)` or `Anchor::Range { block, start, end }` whose ends are
  `Point { node, offset }` in a text inline (raw block) or math row. Kept in
  `Document::remarks`, outside undo.
- `Document::apply` works out what an op changes (`edit_of`) and moves the
  ends (`move_remarks`): in the edited text or row, an end after the change
  shifts; text typed at either end stays out; an end in removed items goes
  to the edge of what replaced them. When an op may drop nodes, a range whose
  ends are gone falls back to its block; whose ends moved to another block
  is re-homed; whose block is gone stays detached (undo brings it back).
  An emptied range falls back to the block. Undo and redo go through `apply`.
- `Editor::add_remark(text, author, date)`: a range of the selection in one
  block; selected blocks: the first; no selection: the top-level block the
  caret is in (the whole theorem). Tests `remarks_follow_edits`,
  `remarks_on_blocks`, `remarks_in_formulas`.
- Known: joining two paragraphs (Backspace at the start of the second) moves
  the second's text into the first's text inline, so a range on it is lost:
  the remark is detached until undo.

## R2b — Remarks saved in the `.sem` (2026-11-13)

- `remarks.json` beside `document.json` (only when there are remarks), part
  of the snapshot, so every save that changes a remark is a version. Each
  remark: id, block id, a range's ends as path + offset from the block (inner
  ids are not saved), text, author, date, resolved. On load a range that no
  longer resolves is on its block; a detached remark's block id is reserved
  (`Document::reserve_id`) so no new block takes it. Old versions open with
  their remarks. Test `remarks_roundtrip`.

## R2c — Adding remarks and seeing them (2026-11-13)

- *Add Remark…* (menu, Ctrl+Alt+M): a dialog with a text field; the remark
  goes on the selection, or on the top-level block the caret is in; author
  as for versions, today's date. `SemEditor::add_remark`.
- Drawn under the text whether focused or not: a range shaded yellow (the
  selection's line rectangles, `range_rects`, now shared); a block remark as
  a yellow bar in the paper's left margin, the block's height (a nested
  block: its caret positions' lines, `block_extent`). Resolved and detached
  remarks are not drawn. Pointing at a shading or bar shows author, date and
  text as a tooltip, until the sidebar (R3).
- Remarks count for unsaved changes (`saved_state`). README: feature and key.
- Drawing is unchecked by tests (widget only): awaiting the user's test.
- Follow-up (2026-11-14): the user saw no margin bar because their remark was on
  a selection, which was only shaded. Now every remark gets the bar: a range's
  spans its shaded lines (`rects_extent`); pointing at it shows the remark too.

## R3a — Remarks for the sidebar, in the model (2026-11-14)

- `Remark::replies` (`Reply`: text, author, date), saved in `remarks.json`
  (`replies`, left out when empty; older files read fine).
- `Document::remarks_in_order`: by top-level block, then where the range starts
  (a whole block first); detached last.
- `Editor`: `reply_remark`, `set_remark_resolved`, `delete_remark` (none part
  of undo, like `add_remark`; refused in an old version); `remarks_at_caret`
  (open remarks whose range holds the caret, ends included, or whose block
  does); `select_remark` (selects the range, or caret at the block's start).
- Tests `remarks_for_the_sidebar`, `remarks_roundtrip` (replies, resolved).

## R3b — The Remarks sidebar (2026-11-14)

- A right sidebar (`adw::OverlaySplitView` at the end, inside the outline's),
  toggled by a header button; shown on opening a document with open remarks
  and after adding one. `window/remarks.rs`, `RemarksPane`.
- A card per remark in text order (`remarks_in_order`): author, date, text,
  replies indented; "Its text was deleted." for a detached one. Open remarks
  first; resolved ones below under *Resolved*, dimmed.
- Clicking a card selects its range (or goes to its block) and focuses the
  editor; on every caret move the cards of `remarks_at_caret` are selected
  and scrolled into view. The pane rebuilds only when the remarks change.
- Buttons: Reply (dialog, as for adding; `ask_remark_text`), Resolve; on a
  resolved card Reopen and Delete. None in an old version's window.
- Not yet: the cards beside their text (they are a list), editing a remark,
  remark changes in undo. Widget only: awaiting the user's test.

## C1 — Chat with Claude (2026-11-14)

- R4 (converting `#audit[…]`) dropped: the user does it by hand.
- `chat.rs`: `Message` (author, text, quote), `Quote` (where, Typst source);
  `chat.json` in the `.sem` beside the snapshot, so not in versions (`to_zip`
  takes the chat, `from_zip` returns it). `command`: `claude -p --output-format
  stream-json --verbose --include-partial-messages --tools "" --strict-mcp-config
  --no-session-persistence --setting-sources "" --system-prompt …`, through
  `bash -lc` (finds `~/.local/bin`), in the Flatpak via `flatpak-spawn --host
  --directory=/tmp` (manifest: `--talk-name=org.freedesktop.Flatpak`). `--bare`
  cannot log in with the subscription (probed). `prompt`: the paper (the
  `.typ` export), earlier messages, the question with its selection; stateless,
  no `--resume`, so nothing depends on Claude Code's session files and the
  paper never meets the argument length limit. `parse_line`: text deltas, the
  result (error text when `is_error`). System prompt: Typst markup and Typst
  math, no LaTeX, no Markdown.
- Right sidebar: Remarks | Chat switcher (`adw::InlineViewSwitcher`), wider:
  40 % of the window, 320–520 px (the user found it too narrow; libadwaita
  1.10's split views cannot be dragged wider, and a `gtk::Paned` would leave
  libadwaita, so not now). `window/chat.rs`, `ChatPane`: messages (the user's in a card,
  the quote dimmed above), input box (Enter asks, Shift+Enter new line), the
  button turns into Stop while Claude writes; the answer streams in. If it fails
  or is stopped before any text, the question goes back into the input box and
  the error shows under it. Clear (with a confirmation). The selection's quote
  (`SemEditor::selection_source`, `Window::selection_quote`: section heading
  and the top-level block's Typst) shows above the input. *Ask Claude…*
  (menu) opens the tab; no shortcut (the user does not want one). Hidden in an old version's window. The chat
  counts for unsaved changes.
- Checked end to end from inside the sandbox with the app's arguments (answer
  in Typst math, no tools, no MCP). The pane is widget only: awaiting the user's
  test. Next C2: render answers with Typst.
