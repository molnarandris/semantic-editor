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
`IMMulticontext`, Backspace, Enter (split), arrows, undo. Text blocks still re-render
through Typst on every edit (debounced 50 ms).

## M5 — Math editing

Structural keys per the table, slots, symbol shortcut table, breadcrumb, structure
highlight. If glyph spans turn out too coarse, use the marker-box fallback from the spec
(serializer wraps nodes in labelled boxes, layout reads their frames).

## M6 — Files and export

`.sem` JSON, NodeIds regenerated on load, `.typ` and PDF export, sourceview5 pane,
GActions + accelerators. Test: save → load → export identical Typst.

Each milestone: `cargo test`, `cargo clippy` clean, then stop for manual testing.
