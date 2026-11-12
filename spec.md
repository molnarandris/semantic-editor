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
- **Outline pane** (user, 2026-11-12): left of the editor (`adw::OverlaySplitView`),
  shown or hidden by a header-bar button: the document's headings, indented by
  level, the one the caret is under selected; a click (or Enter) goes to it.
  Independent of an `#outline()` in the document.
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

Principle (discussed 2026-11-08): **stay close to Typst in what a document means,
diverge freely in how it is typed and shown.** Every structure the editor has maps to
plain Typst and back without loss; what the editor cannot (yet) represent stays raw
Typst (code blocks, raw blocks, generic calls with source arguments), never dropped or
approximated. Conveniences (typing `sum` then `_`, slots that grow as you type, outlines,
hidden code) live only in the editor. Formatting with no structure of its own is not
invented: it is a Typst call or rule. This keeps `.typ` import/export one-to-one and
lets Typst do all layout.

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
- Done 2026-11-08 (see PLAN.md). The marker is drawn by Typst (a dashed bar listing the
  definitions); "Show Code" in the main menu shows the source instead.

### M11 — Inline code: diagrams inside formulas and text

- A new atom for math rows and inline lists holding Typst code (e.g. `#dia({…})`,
  `#box(cetz.canvas({…}))`). It behaves exactly like a diagram block: double click or
  Enter opens the source dialog with the live preview, Backspace selects then deletes.
  The difference is only internal: it lives inside a formula, so editing it recompiles
  the formula's block.
- Inserting: "Insert Diagram" with the caret inside a formula inserts an inline one.
- Done 2026-11-09 (see PLAN.md). In text, an inline diagram is a formula holding only
  the code (`$` then Insert Diagram), not a separate inline kind. Wishlist: typing `#`
  in a formula to start code; code atoms that are not content (`#sym.alpha`) render
  boxed in the editor, a little wider than in the export.

### M12 — Math: generic calls and missing atoms

- The generic call node in math: `cal(A)`, `overline(x)`, `hat`, `bold`, `tilde`, `bb`,
  `abs`, `norm`, `sqrt`, `root`, `upright`, `op`, `underbrace(a, b)`, `cases(…)` (comma
  separated arguments), header macros (`ket(psi)`). Typing: the name alone, resolved
  when the word ends (space, a non-letter, caret moves), as symbol names are today:
  `sqrt` + space gives √ with an empty slot and the caret inside; `sqrt(` works too
  (the parenthesis is absorbed). Typst tells whether the name is a function or a symbol
  (`phi` stays a symbol) and how many content arguments it takes (`root` gets two
  slots). No backslash commands (decided 2026-11-08).
- Atoms: primes (`f'`), text in math (`"if"`), alignment point `&` and line break `\`
  (multi-line aligned equations; Typst lays them out, the caret follows spans).
- `phi(x)` where `phi` is a symbol is not a call (already representable).
- Later: matrices (`mat(…; …)`, 2D arguments).
- `cases` by lines (user request 2026-11-09): Enter in a case starts the next one (as a
  line break would look), ↑/↓ move between cases; `,` still works. Typing only: the
  source has Typst's commas.
- Split 2026-11-09: **M12a** the call node, **M12b** the atoms; both done (see
  PLAN.md). Line break typed as in Typst, `\` then space; an empty new line shows a
  placeholder with the caret in it. While a formula is being edited (caret in it),
  `&` shows as a thin line and a line break as `↵` at the end of its line; finished
  formulas show neither (decided 2026-11-09).

### M13 — Text markup and inline calls

- Bold, italic (as the calls `strong`/`emph`, written `*…*`/`_…_`), inline code, links,
  line breaks, non-breaking space and dash shorthands.
- The generic call in text: `#cl[…]`, `#todo[…]`, `#highlight[…]`, `#underline[…]`,
  `#footnote[…]`: content argument editable in place, rendered by Typst.
- Split 2026-11-09: **M13a** the inline call node (bold, italic, generic calls, links
  as `#link("…")[…]`); **M13b** the atoms (inline code, line breaks, `~`, dashes).
- M13a decisions (2026-11-09): one node, `Inline::Call { name, args, body }`, the body
  a list of inlines like a paragraph's (text, formulas, calls: nesting as in Typst);
  bold and italic are the calls `strong` and `emph`. Typing as in Typst: `*` starts
  bold and `*` again ends it (`_` italic), except right after a letter or digit, where
  Typst reads them as plain text (`snake_case`, `a*b`); `\*` gives a literal `*`.
  `#name[` (or `#name(args)[`) starts any call, `#name` + space too when it is not the
  whole paragraph (then it is a block call, as before); `]` leaves the innermost call.
  Backspace after a call selects it, then deletes it; in an empty call it goes; at
  the start of a full call's body it marks the call (stronger outline), and the
  next Backspace replaces the call by its body, as for environments (user
  request 2026-11-11; it used to move left). Enter
  inside a call splits it with the paragraph. Typst draws everything; export writes
  `*…*`/`_…_` where Typst reads them back the same, else `#strong[…]`.
  While the caret is in a call, it is outlined like an environment (one box per
  line it spans; user request 2026-11-09). Changed 2026-11-11 (user report: the
  boxes of a multi-line call overlapped): one contour around its lines.
- M13b decisions (2026-11-09): inline code `` `…` `` is its own inline (`Inline::Raw`),
  edited in place: `` ` `` starts it and `` ` `` ends it, everything typed in between
  is kept verbatim. A line break is a `\n` in the text (written `\ `), typed as in
  Typst (`\` then space) or with Shift+Enter. `~` is a non-breaking space, `--` an en
  dash, `---` an em dash, stored as the Unicode chars (`\~` and `\-` type the literal
  chars). Not done: soft hyphen `-?`, `...`, smart quotes (Typst already sets them).
  Inline code is outlined like calls while the caret is in it (user request).

### M14 — Nested blocks: theorem environments and lists

User request (2026-11-08, after M10): the theorem's text must be editable in place, like
any paragraph; the environment only contributes "Theorem", the number and the formatting
(bold, italic, …). The body is the call's content argument. Moved before M11 at the
user's request. Split: **M14a** environments with a body of blocks (done 2026-11-08, see
PLAN.md; created by typing `#lemma` + space); **M14b** titles, lists, block-level calls.
Titles decided 2026-11-08: optional, never added by default; positional
`#theorem[Title][…]` as in the popular packages; added by clicking the heading
("Theorem 2.1.") or by typing `#lemma(` instead of `#lemma` + space.

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

- Lists: bullet, numbered and term lists with nesting. Done 2026-11-08 (see PLAN.md):
  items are containers as in Typst (no list node); typed with `- `, `+ `, `/ `; Enter,
  Backspace and Tab/Shift+Tab as in word processors.
- Block-level calls with block bodies (`#claude-block[…]`, `#align(center)[…]`). Done
  2026-11-09 (see PLAN.md), with function detection by parsing the header's and code
  blocks' `#let`s (not compiling): `(` starts a title only for functions taking one,
  else source arguments. Wishlist: edit an existing call's arguments; know package
  functions (would need compiling the header).

### M15 — Labels and references

- **Labels** on headings, display equations (numbered when labelled) and theorems:
  `label: Option<String>` on those blocks. Shown as a pill (drawn by the widget, in the
  right margin, not part of the Typst render). `<` inside math is "less than", so
  labels get a visible slot instead of a shortcut: while the caret is in a labelable
  block, a faint "+ label" pill shows in the right margin (where the equation number
  goes). Clicking the pill puts the caret in it (the arrows pass it by, user
  2026-11-12); typing there edits the label in place, Enter / Esc / ↑ / ↓ leave it.
  Existing labels always show as pills.
- **References**: `@` in text opens a completion popover at the caret listing labels
  (with their kind and number), filtered as you type. New inline node `Ref(label)`,
  an atom for the caret (Backspace selects, then deletes). Dangling refs are flagged.
- **Navigation**: clicking a reference scrolls to its target and puts the caret there;
  Plain click follows the link (the caret reaches a reference with the arrow keys
  instead); Alt+Left goes back.
- Effort: medium–large (model node, popover, label table, resolution).
- Split 2026-11-09: **M15a** labels (model, pill, editing, `<label>` in the source);
  **M15b** references, the popover, numbers and navigation.
- M15a decisions (2026-11-09): the label is a field of every block (`label`, empty for
  none), used on headings, display formulas and environments; written `<label>` after
  the block as in Typst. Typing in the pill keeps only the chars Typst allows in a
  label (letters, digits, `-`, `_`, `.`, `:`); space, Enter or Esc leave
  it (2026-11-12: ←/→ stay in it, and only a click enters it). The pill is drawn
  beside the page, right of the block's first line (a display formula's: centred
  on the formula, at its number's height, 2026-11-12), in a monospace font so every
  char has its caret place. Numbering
  stays Typst's: equations are numbered if the header numbers them (labelling does
  not number an equation by itself, which would need inventing formatting; a header
  can number only labelled equations with a show rule).
- M15b decisions (2026-11-09): a reference is an inline atom, `Inline::Ref(label)`.
  `@` starts one (as in Typst, also right after a letter; `\@` is a literal `@`); label
  chars typed after it go into it, anything else ends it, and a trailing `.` or `:`
  goes back to the text (Typst reads `@eq:abs.` as a reference and a full stop).
  Typst renders the reference: every labelled block reports what its labels point to
  (counter and value), and a block with references gets an invisible stand-in target
  per label (a hidden heading, equation or call of the same environment, with the
  counter set), so the header's supplements, numbering and show rules apply. A
  reference to a missing, unnumbered or duplicate label shows `@label` in red
  instead of failing. Steps: model and typing; rendering; the completion popover;
  click to follow, Alt+← back. Changed after testing (2026-11-09): Ctrl+click follows
  (a plain click is too easy to trigger); references get a light green background in
  the editor so they stand out with any styling.

- Labels on figures and theorems too (all targets exist by now); citations (`@key`)
  with the bibliography.

### M16 — Figures, footnotes, bibliography

- Figures as block calls with a body and a caption (content arguments), labelled.
- Footnotes (inline call, M13) with numbering through the chain.
- Bibliography: embedded (as in the user's paper) or a file (needs project files).
- Split 2026-11-09: **M16a** figures with captions; **M16b** footnotes numbered
  through the chain; **M16c** bibliography and citations (`@key`), numbered in
  document order (each citing block gets the earlier citations as hidden ones
  before it, so Typst numbers them as in the whole document).
- M16a decisions (2026-11-09): a figure is a block call (`#figure ` as for
  environments) whose body ends with a caption: a text block of its own
  (`Block::Caption`), edited in place under the figure and written as the call's
  `caption:` argument, `#figure(caption: [Cap])[…]`. A new figure starts with an empty
  caption; Backspace in an empty caption removes it. Figures are labelled and
  referenced like the other targets.
- M16b decisions (2026-11-09): footnotes stay the inline call `#footnote[…]`
  (M13a). Their counter joins the chain, and a block with a footnote becomes
  counter-sensitive, so numbers run through the document. In the editor a footnote's
  text shows under its own paragraph (where Typst puts it on the block's page,
  measured), edited in place; the export puts it at the foot of the page as usual.
  ←/→ pass the footnote's marker in one step and reach its text after the rest of
  the paragraph, where it is shown (user, 2026-11-12).
- M16c decisions (2026-11-09): the bibliography is a block of its own
  (`Block::Bibliography`), its BibTeX (or Hayagriva YAML) source embedded and edited
  in the source dialog like a diagram ("Insert Bibliography"); written
  `#bibliography(bytes(```bib … ```.text))`, so the source stays verbatim. A
  citation is the reference node with a bibliography key, `@key`, as in Typst; the
  completion list offers the keys too, and Ctrl+click goes to the bibliography.
  Rendering: a citing block gets, before it, the citations of the document's earlier
  blocks as hidden ones, and after it the bibliography hidden, so Typst numbers its
  citations as in the whole document; the bibliography block gets all citations
  hidden before it, so it lists what the document cites. Style: a set rule in the
  header (`#set bibliography(style: …)`).
  Wishlist (user, 2026-11-09; postponed, "for now leave it as it is"): footnote texts
  in a list at the very end of the document, the paragraph showing only the number;
  hovering the number shows the text in a bubble. Open question then: where the
  text is edited (the end list, the bubble, or the paragraph).

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
- M17 decisions (2026-11-10): the header is everything before the first content,
  `#show:` rules included (ctheorems-style `#show: thmrules` must apply to every
  block; a template that adds content shows it in every block, the known limit of
  M9a). What the tree cannot represent stays raw as a whole unit (the paragraph,
  formula, heading or list item holding it), never split. A call standing alone
  becomes an environment when its body holds blocks, starts on a new line or has a
  title; otherwise it is a paragraph holding the call (`#highlight[…]`), which
  reads back the same. The opened document is untitled and saved as `.sem`; it
  counts as unsaved until then, so closing it asks to save (user request).
  Known limits: enumeration items with explicit numbers stay raw; a list written with blank lines between its items (Typst's
  wide list) becomes tight, and one after a blank line under its paragraph moves
  up to it. A block call on lines of its own ends its paragraph (as in Typst).
  A figure whose body is its first argument (`#figure(dia(…), caption: […])`) is
  a figure holding that code as a raw Typst block, so references to it are numbered.
  References inside raw blocks (`@thm:x` in a raw figure's caption) get the same
  stand-in or red `@label` as references in the tree, so the block's preview
  compiles on its own (user report: a red dot instead of the figure). Labels
  attached to raw blocks are numbered too when they mark a heading, an equation
  or a figure of a built-in kind (image, table, raw); others show red.
  Spaces in formulas (2026-11-10, user decision): Typst shows a space written
  next to a bar or quoted text and ignores the others, so the tree keeps a space
  before an atom where it shows, imported and exported as written (`|x|`,
  `3 |x| - 1`, `"Rep" (A)` all import; the paper's raw blocks went from 46 to 30).
  Typing in a formula works like writing Typst: `|` is a tight bar, a space next
  to a bar or text is kept (typed before the next atom, it is held for it), and
  Backspace/Delete take such a space away first. Older `.sem` files get the
  spaces their export wrote.
  Restating (2026-11-10, user report "array is empty"): a raw block naming a label
  as a value (`#restate(<lem:x>, mark: true)`, a function that finds the lemma with
  `query`) compiles in the editor with a hidden copy of the labelled block before
  it, its counters as at the lemma (from the lemma's last render), then its own
  counter prefix. The preview is the function's real output: the lemma's number,
  its equation numbers, "restated" if asked. The copy's references get stand-ins.
  Known limit (left for later, user decision 2026-11-10): a restated lemma with a
  footnote shows the note twice in the editor (Typst prints notes of hidden,
  placed or clipped content whatever the show rules; the export is fine).
  Wishlist: explicit item numbers, wide lists, numbered references to raw environments (custom figure
  kinds).

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
- M18 decisions (2026-11-10): the title is a block of its own (`Block::Title`), at
  the start of the document, holding the fields; its dialog has the fields (not the
  header dialog, which edits source). It shows Typst's `#title()` and an author
  line read from the metadata (`context document.author`); the date is metadata
  only. No title block, no metadata: new documents start without one ("Document
  Title…" adds it). Restyling: `show title` rules in the header. Wishlist: keywords
  and abstract; a date on the page; author affiliations.

### Problems (user request, 2026-11-10)

- **Next Problem** (F8, Shift+F8 back; in the menu): moves the caret to the next
  block with a problem, wrapping around, and names it in a toast ("Problem 2 of 5:
  …"). Problems: a block that failed to compile (the red dot; the first line of
  Typst's error), a reference to a label that does not exist, a reference to a label
  on raw Typst (shown red, without its number), a label defined twice. Citations to
  BibTeX entries written in the document (a raw bibliography) are fine. Known limit:
  labels made in code (`label("…")`) are not seen, so references to them are
  reported. Wishlist: a list of all problems to pick from.

### Options of math calls (user request, 2026-11-11)

Raw Typst comes in two sorts: source by design (diagrams, code, `#restate`, the
bibliography), edited in the source dialog, and text or formulas the tree cannot
hold yet, which display right but are not editable in place. The paper's
`lr(size: #55%, …)` formulas were of the second sort.

- Typst's math functions take named options besides their content: `lr`, `abs`,
  `norm`, `floor`, `ceil`, `round`, `stretch` (`size`), accents (`size`,
  `dotless`), `frac` (`style`), `op` (`limits`), `cancel`, `cases`/`vec`/`mat`
  (`delim`, `gap`, …). A math call keeps them as Typst source (`opts`), written
  before its content arguments.
- **(a)** the tree, import, export. **(b)** the options tab: while the caret is in a
  call with options (or a call that takes them), a small tab on the top edge of
  the call's outline shows them (`size: 55%`), drawn over the render so nothing
  shifts. ↑ from the call's arguments (when nothing in them is above, like a
  numerator over a denominator) or a click reaches it; ←/→ skip it (user
  request after testing) and, in it, stay in it; ↓, Enter or Esc go back into
  the first argument. Typing edits the source; emptied, the options go. No shortcuts (user
  decision). `lr` is typed like `sqrt`: `lr` + space gives `lr(▯)`, the brackets
  and content go inside (in an `lr`, `(` is a bracket). The tab shows "+ options"
  while empty; options are typed as code (`size: 80%`, no `#`), Enter or Esc
  leave into the first argument. Calls with a tab: Typst's math functions with
  options (`lr`, `abs`, the accents, `cases`, …), or any call with options. **(c)** the partner of a bracket next to the caret is
  highlighted (plain and `lr` pairs alike).
- Plain brackets stay as typed (Typst pairs them, `(a]` included); existing ones
  cannot be turned into an `lr` pair, since the partner may not be the intended
  one. Wishlist: `lr` around a selection, once formulas have selections.
- Known limit (measured 2026-11-11): code in a formula (`bigbx` from the header,
  a large operator with limits) is boxed in the editor so the caret finds it; the
  box loses its limits, its centring on the axis and the height `lr` scales to,
  so the editor shows it differently from the export. Putting `limits`/`class`
  back around the box fixed the limits, not the bracket size. User decision: they
  avoid such header operators; built-ins (`limits(⊠)`, `op(⊠, limits: #true)`)
  render the same in both.


### Selection, copy and paste (user request, 2026-11-11)

Asked for overnight; decisions taken by Claude (to be confirmed by the user).

- **3a Selecting**: Shift + a caret move (←/→/↑/↓, Home/End, Ctrl+Home/End),
  Shift+click and dragging with the mouse select from the anchor to the caret.
  The range is normalized to whole pieces (`Selection::normalized`): chars of
  one text, nodes of one math row (a structure whose two children are reached
  is taken whole), inlines of one inline list (a paragraph's or a call's body;
  formulas, calls, references covered whole), and **whole top-level blocks**
  when the ends are in different blocks (precise cross-paragraph ranges:
  wishlist). Drawn as a light band per line, under the text. Backspace/Delete
  delete it, typing and Enter replace it (selected math nodes still wrap with
  `/`, `^`, `_`), Esc cancels, one undo step restores it.
- **3b Copy/Cut** (Ctrl+C/X): the selection as Typst source, written as the
  export writes it.
- **3c Paste** (Ctrl+V): the clipboard text read as Typst: in a formula as
  math, in text as inlines when it is one paragraph, else as blocks.

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
