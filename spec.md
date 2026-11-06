# Semantic math editor — prototype spec


## Goal and scope

Build a native GNOME prototype of a TeXmacs-style semantic editor for math-heavy notes: a tree-based document rendered by Typst, edited directly on the rendered page with a structural caret inside formulas.

The prototype is a foundation, not a throwaway. The model, serializer and compile layers must survive; the widget layer may be rewritten once.

**In scope**

- Paragraphs of text with inline and display math.
- Math nodes: symbols, identifiers, fractions, superscripts, subscripts, empty slots.
- Structural keys (`$`, `/`, `^`, `_`, Tab, Escape), arrows, Backspace, undo/redo.
- Per-block Typst rendering with caret, selection and highlight overlay.
- Status bar with the node path (paragraph › math › fraction › numerator).
- Save/load (JSON) and export to `.typ` and PDF.

**Out of scope for now**

- CeTZ diagram nodes (milestone 6), vector rendering, IME polish, accessibility, theorem environments beyond a simple block, references and numbering, a Typesetter fork.

## Tech stack and constraints

Rust with gtk4-rs and libadwaita, rendering through the `typst` compiler crates. Target GNOME on Linux only.

| Area | Choice | Note |
| --- | --- | --- |
| Language | Rust, stable | Cargo workspace |
| UI | gtk4-rs + libadwaita (current stable) | follow GNOME HIG |
| Typesetting | `typst`, `typst-render`, `typst-pdf` | pin one exact version |
| Helpers | `typst-kit` for fonts/packages if it fits | else hand-roll `World` |
| Optional pane | `sourceview5` | read-only Typst source view |
| Serialization | `serde` + `serde_json` | `.sem` files are JSON |
| License | GPL-3.0-or-later | keeps a later Typesetter merge possible |

Hard constraints:

- Never block the GTK main thread on a compile.
- `model` crate depends on neither GTK nor Typst.
- The tree is the source of truth; Typst source is always derived.

## Workspace architecture

Cargo workspace with four crates:

```
app ──► editor-widget ──► typst-bridge ──► model
 │            │                 │            (serde only; no GTK, no Typst)
 └────────────┴──► gtk4-rs + libadwaita
                                └──► typst crates (pinned)
```

- `model`: tree, cursor, ops, undo. Source of truth.
- `typst-bridge`: serialize, compile, cache, render, span → NodeId.
- `editor-widget`: `SemEditor` widget: drawing, caret, input, hit-testing.
- `app`: window, header bar, status bar, files, export.

Arrows point from a crate to what it uses. Keeping `model` pure lets the widget be rewritten, or moved into a Typesetter fork, without touching the core.

## Document model (`model` crate)

A typed tree with a stable `NodeId` (u64, never reused) on every node. No GTK, no Typst.

```rust
enum Block { Heading { level: u8, inlines: Vec<Inline> }, Paragraph(Vec<Inline>), DisplayMath(MathRow), RawTypst(String) }
enum Inline { Text(String), Math(MathRow) }
struct MathRow(Vec<MathNode>);
enum MathNode { Sym(String), Ident(String), Num(String), Frac { num: MathRow, den: MathRow }, Scripts { base: MathRow, sup: Option<MathRow>, sub: Option<MathRow> }, Slot }
```

- The real types wrap each node with its `NodeId`; the enums above show shape only.
- An empty `MathRow` renders as a `Slot` placeholder.
- Cursor = `CursorPath { block: NodeId, path: Vec<Step>, offset: usize }`, where `Step` names a child (e.g. `Num`, `Den`, `Sup`, `Index(i)`). Never a source byte offset.
- Selection = anchor + head cursors, normalized to whole subtrees inside math.
- All mutations go through `Op` values (`Insert`, `Delete`, `Wrap`, `Unwrap`, `Replace`) applied by one `apply(op) -> inverse_op` function. Undo/redo = stack of inverse ops.
- Unit tests: every op + its inverse round-trips; cursor stays valid after every op.

## Serializer (`typst-bridge` crate)

`serialize(block) -> (String, SpanMap)` turns one block into a standalone Typst snippet plus a map from `NodeId` to byte range.

- Fractions as `(a)/(b)`, scripts as `base^(sup)_(sub)`; always parenthesize to keep structure unambiguous.
- `Slot` → a visible placeholder, e.g. `#box(stroke: (dash: "dashed"), inset: 1pt)[$space$]`, so empty slots have geometry.
- Identifiers of more than one letter must be quoted (`"abc"`) or emitted as Typst names only when they are known symbols.
- Escape Typst-special characters in `Text`.
- `RawTypst` is emitted verbatim.
- Whole-document export = preamble + blocks joined.
- Unit tests: golden snapshots of Typst output, and every snippet compiles without errors.

## Typst bridge: compile, cache, render

One long-lived `World` compiles each block as its own tiny document at the current page width, on a worker thread.

- **World:** in-memory main file per block; system + bundled fonts loaded once; package fetch for `@preview` (CeTZ later).
- **Per-block document:** `#set page(width: <W>pt, height: auto, margin: 0pt)`, shared text/math font settings, then the snippet.
- **Worker:** a channel of `CompileJob { block_id, generation, source }`. Drop superseded jobs per block. Results return to the main loop via `glib` channel/`spawn_future_local`.
- **Cache:** key = hash(source, page width, typst version, zoom). Value = rendered texture + `BlockLayout`.
- **BlockLayout:** size, baseline, and a flat list of `(NodeId, Rect)` boxes, built by walking `Frame` items and mapping each item's `Span` → byte offset → `NodeId` via the `SpanMap`.
- **Render:** `typst-render` → RGBA pixmap → `gdk::MemoryTexture`, at device scale factor. Vector rendering later behind the same interface.
- **Errors:** an incomplete or failing block keeps its last good render plus a small error marker; never blocks typing.
- **Bench:** a CLI `bench` binary times cold and warm compiles of sample formulas (and later the user's heaviest CeTZ diagram).

## Editor widget (`editor-widget` crate)

A custom `gtk::Widget` subclass (`SemEditor`) that stacks block textures vertically and draws the caret and highlights on top. Not built on `GtkTextView`.

- **Layout:** blocks stacked with paragraph spacing; widget width = page width + margins; height = sum of block heights.
- **`measure()` / `size_allocate()`:** report total height; a width change triggers re-compile of all blocks.
- **`snapshot()`:** append each visible block texture; then the current-structure highlight (accent tint), dashed slot outlines if not drawn by Typst, selection rects, and a 1.5 px caret.
- **Scrolling:** implement `gtk::Scrollable` (vadjustment) and place it in a `GtkScrolledWindow`.
- **Hit-testing:** point → block → smallest `BlockLayout` box containing it → `NodeId` → `CursorPath`.
- **Caret geometry:** from the box of the node at the cursor (left/right edge by offset); empty slot = its placeholder box.
- **Input:** `EventControllerKey` for keys, `GestureClick` and `GestureDrag` for click/selection, `gtk::IMContext` (`IMMulticontext`) for committed text.
- **Clipboard:** `gdk::Clipboard`, copy as JSON subtree + plain Typst text.
- **Signals:** `cursor-path-changed` for the status bar, `modified` for the window title.

## Editing engine and keybindings

Keys map to `Op`s in the `model` crate; the widget only translates events. Behaviour mirrors TeXmacs/LyX.

| Key                       | Context            | Effect                                                               |
| ------------------------- | ------------------ | -------------------------------------------------------------------- |
| `$`                       | text               | insert inline math, caret in its slot                                |
| `$$`                      | text               | insert display math block                                            |
| `$` or `Escape`           | math, top level    | leave math, caret after it                                           |
| `/`                       | math               | wrap previous atom (or selection) as numerator, caret in denominator |
| `^` / `_`                 | math               | add sup / sub to previous atom, caret inside                         |
| `Escape`                  | inside a structure | move caret out, just after it                                        |
| `←` `→`                   | anywhere           | move by atom; enter and leave structures                             |
| `↑` `↓`                   | fraction           | numerator ↔ denominator; else previous / next line                   |
| `Backspace`               | after a structure  | first press selects it, second deletes                               |
| `Backspace`               | empty slot         | remove slot and unwrap parent if it becomes trivial                  |
| `Ctrl+Z` / `Ctrl+Shift+Z` | anywhere           | undo / redo                                                          |
| `Enter`                   | text               | split paragraph                                                      |

- Typed letters in math become `Ident`, digits `Num`, operators `Sym`.
- Symbol shortcuts via a table: `->` → →, `<=` → ≤, `\alpha` + space → α, etc.
- "Previous atom" = last `MathNode` before caret in the current row.

## App shell and file format (`app` crate)

An `adw::Application` with one `adw::ApplicationWindow`, matching the notes.sem mockup.

- **Header bar:** new,  title `<file> — Semantic editor`, toggle source pane, main menu (open, save, save as, export .typ, export PDF).
- **Body:** `SemEditor` in a `GtkScrolledWindow`; optional right pane with a read-only `sourceview5::View` showing the serialized Typst, current node highlighted.
- **Status bar:** breadcrumb of the cursor path (clickable to select that structure) and a hint like "Tab → next slot".
- **File format `.sem`:** JSON `{ "version": 1, "typst": "<pinned version>", "blocks": [...] }` via serde; NodeIds regenerated on load.
- **Export:** `.typ` via serializer; PDF via `typst-pdf` from a full-document compile.
- **Actions:** `GAction`s with accelerators (`Ctrl+S`, `Ctrl+O`, `Ctrl+E` export, `F9` toggle source pane).

## Milestones and acceptance criteria

Build in this order; each milestone ends with `cargo test` and `cargo clippy` clean and a working binary.

1. **Benchmark spike.** `bench` binary compiles and renders a formula block via `World`.
   - Done when: prints cold and warm compile+render times for 5 sample formulas.
2. **Model and serializer.** `model` and `typst-bridge` serializer with `SpanMap`.
   - Done when: op/inverse round-trip tests and golden Typst snapshots pass; every snapshot compiles.
3. **Viewer.** App window shows a hard-coded document rendered per block, scrolls, re-renders on resize.
   - Done when: compiles run off the main thread; UI never freezes.
4. **Prose editing.** Caret, click-to-position, typing, Backspace, Enter, arrows, undo in text paragraphs.
   - Done when: caret lands on the clicked character within one glyph.
5. **Math editing.** All keys in the table above; slots; status-bar breadcrumb; structure highlight.
   - Done when: a user can type `S <= (log chi)/(2) + c` purely with structural keys, and the caret is visibly inside the right slot at every step.
6. **Files and export.** `.sem` save/load, export `.typ` and PDF, source pane.
   - Done when: save → load → export yields identical Typst.
7. **CeTZ diagrams (later).** Diagram block with CeTZ source, cached render (memory + disk), split-view edit with debounce.
   - Done when: editing prose next to 20 diagrams triggers zero diagram recompiles.

## Risks, fallbacks, open questions

The main risk is caret precision inside math, because Typst frames carry spans on glyphs but not the math tree's boxes.

| Risk | Fallback |
| --- | --- |
| Spans too coarse to box subexpressions | wrap each node in a zero-cost marker (e.g. `#box` with a label) and read its frame; else write an own math box layout for the editing view only |
| Incomplete formulas fail to compile | always emit slots; on error keep last good render |
| Compile too slow per keystroke | per-block compile, debounce 50–150 ms, cache; measure in milestone 1 |
| Typst internal API changes | pin version; isolate all Typst calls in `typst-bridge` |
| Raster blurry when zooming | re-render at new scale; vector path later |

Open questions:

- Which Typst version to pin (latest stable at start).
- Fonts: New Computer Modern bundled, or system fonts?
- Should `.sem` later become a restricted `.typ` subset for interop?

## Instructions for Claude Code

Plan first, then build milestone by milestone, stopping after each for a manual test.

- Read this spec fully; produce a plan per milestone before writing code.
- Check the pinned Typst crate APIs (`World`, `Frame`, `Span`, `typst-render`) against docs.rs before coding; do not guess signatures.
- Keep `model` free of GTK and Typst; put all Typst calls in `typst-bridge`.
- Write tests with each milestone; run `cargo test` and `cargo clippy` before declaring done.
- After each milestone, stop and list what to try by hand.
- Ask before changing the architecture or adding dependencies not listed here.

## References

- [Typesetter](https://codeberg.org/haydn/typesetter): Rust/GTK Typst editor; reuse ideas for `World`, packages, compile queue.
- [Typst](https://github.com/typst/typst): `typst-ide` crate for span/jump mapping.
- [Tinymist](https://github.com/Myriad-Dreamin/tinymist): incremental compile loop.
- [Rnote](https://github.com/flxzt/rnote): custom canvas widget in gtk4-rs.
- [LyX](https://www.lyx.org) `mathed`: structural math cursor logic.
- [gtk-rs book](https://gtk-rs.org/gtk4-rs/stable/latest/book/): subclassing and widgets.
