# Implementation plan

Pinned: Typst **0.15.1** (`typst`, `typst-layout`, `typst-render`, `typst-pdf`, `typst-kit`),
gtk4-rs 0.11, libadwaita 0.9, sourceview5 0.11. Rust 2024 edition.

Decisions on the spec's open questions (change any time):

- Fonts: the embedded Typst fonts only (New Computer Modern + NCM Math, Libertinus,
  DejaVu Sans Mono) via `typst-kit/embedded-fonts`. Deterministic, no fontconfig.
  System fonts can be added later behind the same `World`.
- `typst-layout` is added as a direct dependency: `PagedDocument` lives there and
  is not re-exported by `typst`. Same pinned version; Typst-only, inside `typst-bridge`.
- `today()` returns `None` for now (no `chrono`).

## Build and packaging (Flatpak only)

App ID `io.github.molnarandris.SemanticEditor` (`.Devel` suffix for dev builds), GNOME
Platform/SDK 51 + `rust-stable//26.08`; gtk4-rs with `v4_18`, libadwaita with `v1_8`.

- Manifest: `build-aux/io.github.molnarandris.SemanticEditor.Devel.json` (GNOME Builder
  picks it up). Network allowed during build for now; Flathub later needs
  `cargo-sources.json` from `flatpak-cargo-generator`.
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

Each milestone: `cargo test`, `cargo clippy` clean, then stop for manual testing.
