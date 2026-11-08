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

- **Header bar:** new,  title `<file> — Semantic editor`, main menu (open, save, save as, export .typ, export PDF).
- **Body:** `SemEditor` in a `GtkScrolledWindow`; optional right pane with a read-only `sourceview5::View` showing the serialized Typst, current node highlighted.
- **Status bar:** breadcrumb of the cursor path (clickable to select that structure) and a hint like "Tab → next slot".
- **File format `.sem`:** JSON `{ "version": 1, "typst": "<pinned version>", "blocks": [...] }` via serde; NodeIds regenerated on load.
- **Export:** `.typ` via serializer; PDF via `typst-pdf` from a full-document compile.
- **Actions:** `GAction`s with accelerators (`Ctrl+S`, `Ctrl+O`, `Ctrl+E` export).

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

## Wishlist after M7 (not started)

Requested 2026-11-06. Grouped into proposed milestones; open questions marked **Q**.

**Shared design problem.** Every block compiles as its own document, so Typst counters
and labels do not cross blocks: every heading would be "1", and `@label` fails in a
block that does not contain the label.

Chosen approach: **counter state chained through blocks** (no whole-document compile).

- Every block has a *start state* (values of the tracked counters) and an *end state*.
  A counter-sensitive block's snippet begins with `#counter(..).update(..)` for its
  start state and ends with a `#context metadata(..)` reporting the end state, which the
  bridge reads from the compiled document. End state of block k = start state of k+1.
- Tracked counters: `heading`, `math.equation`, and those the header uses (custom
  `counter("…")`, figure kinds), found by asking the header or scanning it.
- Only blocks that read or change counters (headings, labelled equations, theorems,
  raw Typst, blocks with references) get the prefix and suffix. Plain paragraphs keep
  their snippet unchanged, so they stay cached and never recompile.
- The start state is part of a block's render key. After an edit, recompile that
  block; if its end state changed, recompile the next counter-sensitive block, and so
  on. Stop as soon as an end state comes out unchanged. Inserting a heading at the top
  recompiles the later headings and theorems (a few ms each), nothing else.
- References: the target block also reports how a reference to each of its labels reads
  (e.g. "Theorem 2.1", formatted by the header's own rules). A block that references it
  emits that text as a link and recompiles only when the text changes.
- Whatever the header says (numbering patterns, theorem definitions, shared counters)
  applies without the editor understanding it.
- Limits: anything that needs the whole document at once (outline, "last page",
  forward-looking `query`) is not exact in the editor; export is a normal full compile.
  A test compares the chained numbers against a full-document compile.

### M8 — Cleanup and diagram editor (items 7, 8)

- **Remove the source pane** (F9, header toggle, `source_view.rs`). It was temporary.
- **Diagram/raw editing side by side**: source on the left, live render on the right.
  Proposed: an `adw::Dialog` (or window) with a GtkSourceView and a preview picture fed
  by the same worker. Simpler than doing it in-page, since the block can be anywhere on
  the page. Effort: small. The same dialog edits the document header in M9a.
- Later: Typst syntax highlighting in the source dialog (GtkSourceView 5 has no Typst
  language). Borrow the language definition from Typesetter (check its license).

### M9a — Header and numbering engine (items 1, 4)

Order decided 2026-11-07: M8 → M9a → M10 → M11 → M9b. The counter chaining is the
riskiest piece and M10/M11 depend on it, so it comes before the header UI polish.
Changed 2026-11-08: replaced by the "Roadmap to full Typst" below (M10–M18).

- **Editable header**: the document preamble (set/show rules, imports, theorem
  definitions), stored in `.sem`, prepended to every block snippet and to the export.
  Edited through the M8 source + preview dialog, opened from a menu entry (the banner
  comes in M9b).
- **Counter state chained through blocks** as in the shared design above, with the test
  that compares chained numbers against a full-document compile.
- **Numbered headings**: number shown, not editable. The number is part of the render
  but has no node, so the caret and hit-testing skip it.
- Effort: medium–large (mostly the chaining).
- Done 2026-11-08 (see PLAN.md). Decisions: the editor's fonts stay as a fixed preamble
  before the header (a header may override them); new documents get
  `#set heading(numbering: "1.1")`; old files load with an empty header; the header
  dialog previews a fixed specimen of text.
- Known limits: a header that produces content (e.g. a `#show: template` that adds a
  title) shows it in every block (M9b's title block will address this); figure counters
  and counters defined inside packages are not tracked yet (M10).

## Roadmap to full Typst (decided 2026-11-08)

Goal: edit everything a Typst document can contain; only real code (scripting, layout
commands without content) stays raw Typst. The order follows what builds on what; the
user needs all of it before switching to the editor, so the `.typ` import comes last
(replacing the earlier plan M9c → M10 → M11 → M9b).

Input: an inventory of the user's paper (`examples/inventory.rs`; counts only). Its
needs, roughly by frequency: math functions (`cal` 762, `overline` 417, `hat`, `bold`,
`tilde`, `bb`, `abs`, `lr`, header macros like `ket`; ~1500 in all), references (271) and
labels (110), CeTZ drawings inside formulas (101), theorem environments (~70, built as
figures with a kind), ~100 document-level `#let`s next to the figures that use them, a
mid-document `#set heading(numbering: …)` (appendix), primes (129), text in math (90),
lists (90 items), text markup (~100), multi-line aligned equations (30), `cases`,
figures, footnotes, an embedded bibliography. Not used: images, tables, local files.

A shared idea: **one generic call node** (function name, editable content arguments,
other arguments kept as source). Typst draws it, so the editor never needs to know what
`cal`, `overline`, `ket`, `lemma` or `cl` look like; the caret moves through the content
arguments via spans, like numerator and denominator today. Works for the header's own
functions and packages.

### M10 — Code blocks (definitions and rules in the body)

- A raw Typst block whose top level is only `#let`/`#set`/`#show`/`#import`/comments is a
  **code block** (detected by parsing; no user decision). Its source is compiled before
  every later block (in document order, after the header), so definitions and rules
  apply from that point on, as in Typst. A raw block with content stays a content block.
- Shown collapsed by default: a thin marker with a distinct style (e.g. a margin chip
  "{ }" or a dashed line) saying that code is hidden there; double click or Enter opens it
  in the source dialog, like a diagram. A view option shows code blocks expanded.
  (The margin marker is shared UI with label pills, M15.)
- Editing a code block recompiles the blocks after it (like a header change); cache
  keys include the accumulated code.
- Also: figure counters (`counter(figure.where(kind: …))`) in the counter chain, for
  theorems built as figures.

### M11 — Inline code: diagrams inside formulas and text

- A new atom for math rows and inline lists holding Typst code (e.g. `#dia({…})`,
  `#box(cetz.canvas({…}))`). It behaves exactly like a diagram block: double click or
  Enter opens the source dialog with the live preview, Backspace selects then deletes.
  The difference is only internal: it lives inside a formula, so editing it recompiles
  the formula's block.
- Inserting: "Insert Diagram" with the caret inside a formula inserts an inline one.

### M12 — Math: generic calls and missing atoms

- The generic call node in math: `cal(A)`, `overline(x)`, `hat`, `bold`, `tilde`, `bb`,
  `abs`, `norm`, `sqrt`, `root`, `upright`, `op`, `underbrace(a, b)`, `cases(…)` (comma
  separated arguments), header macros (`ket(psi)`). Typing: name then `(` creates the
  call with an empty slot (`sqrt(`); the editor asks Typst whether the name is a
  function (call) or a symbol (`phi(x)` stays symbol + parenthesis). No backslash
  commands (decided 2026-11-08).
- Atoms: primes (`f'`), text in math (`"if"`), alignment point `&` and line break `\`
  (multi-line aligned equations; Typst lays them out, the caret follows spans).
- `phi(x)` where `phi` is a symbol is not a call (already representable).
- Later: matrices (`mat(…; …)`, 2D arguments).

### M13 — Text markup and inline calls

- Bold, italic (as the calls `strong`/`emph`, written `*…*`/`_…_`), inline code, links,
  line breaks, non-breaking space and dash shorthands.
- The generic call in text: `#cl[…]`, `#todo[…]`, `#highlight[…]`, `#underline[…]`,
  `#footnote[…]`: content argument editable in place, rendered by Typst.

### M14 — Nested blocks: theorem environments and lists

- Theorem, lemma, definition, proof, … as a container block holding paragraphs and
  equations (the model is a flat block list today, so nesting is the main change:
  cursor paths, ops, Enter/Backspace at the edges, Escape leaves the environment).
- The whole environment compiles as **one** render unit (`#theorem[para \ para $ … $]`),
  so the header's border and "Theorem 2.1 (Title)." come out exactly as in the export.
  Spans still map glyphs to nodes, so the caret works inside. Editing inside recompiles
  the theorem (small). "Theorem 2.1" and the proof's ∎ have no node, so they are not
  editable; the optional title is an editable field.
- Theorems are defined in the header by the user's own functions (packages maybe later),
  some sharing counters. The editor knows only a list of environment names → header
  function (e.g. `theorem`, `lemma`, `proof`), and emits `#theorem[…]` / `#theorem(title)[…]`.
  The rendered "Theorem 2.1." and the border come from that function. The counters come
  from the chained state above.
- The editor **detects the environments from the header** (which the user edits): the
  functions it defines, e.g. via the header's `#let`s, checked by compiling the header
  and keeping those that take a body. The list updates when the header changes; nothing
  to declare separately. Environments whose definition disappears keep their content
  and show an error marker.
- Effort: large; the biggest item.

- Lists: bullet, numbered and term lists with nesting, same container mechanism.
- Block-level calls with block bodies (`#claude-block[…]`, `#align(center)[…]`).

### M15 — Labels and references

- **Labels** on headings, display equations (numbered when labelled) and theorems:
  `label: Option<String>` on those blocks. Shown as a pill (drawn by the widget, in the
  right margin, not part of the Typst render). `<` inside math is "less than", so
  labels get a visible slot instead of a shortcut: while the caret is in a labelable
  block, a faint "+ label" pill shows in the right margin (where the equation number
  goes). → at the last position of the block moves the caret onto the pill; typing
  there edits the label in place, → / Enter leave it. Clicking the pill works too.
  Existing labels always show as pills.
- **References**: `@` in text opens a completion popover at the caret listing labels
  (with their kind and number), filtered as you type. New inline node `Ref(label)`,
  an atom for the caret (Backspace selects, then deletes). Dangling refs are flagged.
- **Navigation**: clicking a reference scrolls to its target and puts the caret there;
  Plain click follows the link (the caret reaches a reference with the arrow keys
  instead); Alt+Left goes back.
- Effort: medium–large (model node, popover, label table, resolution).

- Labels on figures and theorems too (all targets exist by now); citations (`@key`)
  with the bibliography.

### M16 — Figures, footnotes, bibliography

- Figures as block calls with a body and a caption (content arguments), labelled.
- Footnotes (inline call, M13) with numbering through the chain.
- Bibliography: embedded (as in the user's paper) or a file (needs project files).

### M17 — Import `.typ` files

- Parse with `typst::syntax` (no compile needed). The leading part of the file (set/show
  rules, imports, `#let`s before the first content) becomes the header.
- Map everything the model knows (by then: M10–M16). What is left (scripting, layout
  commands without content, `#show: template`) becomes a raw Typst block with its source
  verbatim, so nothing is lost and it still renders. No "raw math" fallback: the user
  wants full support (decided 2026-11-08).
- Acceptance: importing then exporting gives a document that compiles to the same
  PDF (compare page images) for the user's own files; unknown parts round-trip
  byte for byte.
- Decided 2026-11-08: one-way import (open `.typ`, save as `.sem`, export `.typ` as
  before). Saving back into the original `.typ` is not planned for now.
- With M10–M16 done, the header gets the leading definitions, code blocks keep the
  mid-document ones in place, and only real code ends up raw.

### M18 — Title fields

- ~~Header banner~~ (rubber-band overscroll): dropped 2026-11-08; the header dialog is
  enough.
- **Title, authors, date**: structured fields (in the header dialog; title, authors,
  date, maybe keywords/abstract), stored in `.sem`. The serializer emits them as
  `#set document(title: …, author: (…), date: …)`, which is the metadata (PDF info).
  What appears on the page is up to the header: the default header shows the title
  block (`#title()` plus an author line); the user can restyle or remove it. In the
  editor the title block is a rendered, non-editable block at the top of the page;
  clicking it opens the header dialog at that field.


### Later

- Project files: images, `#include`, local imports, bibliography files.
- Multi-file projects: a zip holding the document, its files and a git repository for
  version control (idea, 2026-11-08; compare Typesetter's project format).
- Matrices, tables (2D content arguments).

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
