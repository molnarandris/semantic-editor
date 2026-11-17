# Semantic editor — specification

What the editor is and how it behaves. Features and keys for users are in
[README.md](README.md); how it is built, and why, in [NOTES.md](NOTES.md); what is
next in [TODO.md](TODO.md). Bugs are GitHub issues.

## Goal

A native GNOME editor for math-heavy notes and papers in the spirit of TeXmacs and
LyX: a tree-based document, typeset by Typst and edited directly on the typeset page,
with a structural caret inside formulas. It should be able to edit everything a Typst
paper contains.

**Principle: stay close to Typst in what a document means; diverge freely in how it
is typed and shown.**

- Every structure the editor has maps to plain Typst and back without loss. What it
  cannot represent stays raw Typst (code blocks, raw blocks, calls with source
  arguments), never dropped or approximated. `.typ` import and export are one to one.
- Typst does all layout and all drawing of content. The editor never imitates a
  function's look: a call it does not know is rendered by Typst like any other.
- No formatting of the editor's own invention: formatting is a Typst call or rule.
- Conveniences (slots, outlines, hidden code, greedy line breaking while typing,
  marks for `&` and line breaks) exist only in the editor, never in the export.

## Constraints

- Rust (stable, 2024 edition), gtk4-rs and libadwaita, GNOME on Linux only, shipped
  as a Flatpak only. License GPL-3.0-or-later.
- Typst crates pinned to one exact version; all Typst calls in `typst-bridge`.
- `model` depends on neither GTK nor Typst. The tree is the source of truth; Typst
  source is always derived from it.
- The GTK main thread never waits for a compile.
- No new keyboard shortcuts for new features: menus and sidebars instead (the user's
  choice). Existing keys stay.

## Architecture

```
app ──► editor-widget ──► typst-bridge ──► model
```

- `model`: the tree, caret paths, ops with inverses, undo, the editing commands,
  remarks. Typst knowledge it needs (symbols, what a function takes) comes in as
  plain function pointers.
- `typst-bridge`: serializer (tree → Typst, with a span map), importer (Typst → tree,
  by the parser, no compile), compile worker and caches, layout (frame → node boxes),
  counters, project files.
- `editor-widget`: the `SemEditor` widget: drawing, caret stops, input, hit testing,
  compile scheduling.
- `app`: windows, menus, sidebars (outline, versions, remarks, chat), files, history,
  dialogs.

## Document model

A typed tree; every node has a `NodeId`. Block ids are saved, so remarks can point
at blocks; inner ids are regenerated on load.

- **Blocks**: paragraph, heading, display formula, environment / block call
  (`#name(args)[title][body]`, body of blocks), list item (bullet, numbered, term;
  lists are consecutive items, nesting is items in items, as in Typst), figure
  caption, picture (`image` with options), diagram (CeTZ source), bibliography,
  title block, raw Typst (code blocks are raw blocks of only
  `let`/`set`/`show`/`import`). Any labelable block has a label.
- **Inlines** (alternating with text as in a paragraph): formula, call
  (`#name(args)[body]`; bold and italic are `strong` and `emph`), inline code
  (`` `…` ``), code piece (`#h(1em)`), reference or citation (`@label`).
- **Math**: identifiers, numbers, symbols, fractions, scripts, primes, text, `&`,
  line breaks, groups, calls (`name(args)` with named options kept as source),
  code atoms, inline diagrams, slots. A space before a node is kept where Typst
  shows it (next to a bar, text or a box).
- All changes go through invertible ops; undo is a stack of inverses.
- Caret: `CursorPath { block, path, offset }`, never a source offset.

## Rendering

- Each top-level block compiles as its own small document at the page width, on one
  worker thread; renders are cached by source, width and scale (diagrams and raw
  blocks also on disk). Only changed blocks recompile.
- Around each block: the editor's preamble, the header and the code blocks before it,
  then what makes it render as in the whole document:
  - **Counters** (headings, equations, footnotes, figure kinds, the header's own) are
    chained: a counter-sensitive block starts from the previous one's end state and
    reports its own.
  - **References** get an invisible stand-in target with the right counter, so the
    header's supplements and numbering apply; unresolvable ones show a red `@label`.
  - **Citations**: earlier citations hidden before the block, the bibliography hidden
    after it, so numbering and style are the document's.
  - **Restated blocks** (`#restate(<lem:x>)`) get a hidden copy of what they name.
- Environments and block calls compile as one unit, so they look exactly as exported.
- A block that fails keeps its last good render and a red dot. A code piece or call
  that fails shows its source on red instead, editable in place.
- The block being typed in breaks its lines greedily until the caret leaves it.

## Editing

Input follows Typst: what you type is what you would write in Typst, turned into
structure as soon as it is unambiguous.

- **Text**: `*`/`_` start and end bold/italic (not right after a letter or digit);
  `` ` `` inline code; `~`, `--`, `---`; `\` + space or Shift+Enter a line break;
  `\` before a special char types it literally. Spaces Typst would collapse are
  never stored.
- **Calls and code**: `#name[` or `#name ` makes a call when the function takes
  content (from the header's `#let`s, imports or Typst's library); as a whole
  paragraph, a block call or environment (`#lemma(` with a title). `#code ` that is
  not such a call (`#h(1em)`, `#sym.dagger`, a name defined nowhere) becomes a code
  piece, shown as what it draws and opened as source when the caret is in it.
  `#name` being typed shows in the code font. Undo right after a conversion leaves
  the text.
- **Math**: `$` inline, `$$` display, `$` at a formula's start toggles the two.
  Letters form identifiers; a finished word becomes a Typst symbol, call or
  operator, else code (as in Typst: a typo fails and shows on red, the header's
  names work). Text is only typed with `"`, and edited in place like code. `/`, `^`,
  `_` take the previous operand; `(` groups; `,` moves between arguments; `'`
  primes; `"` text; `&` and `\` + space for aligned lines. Tab visits empty slots.
  From the second letter of a name, a list offers Typst's symbols (with their
  variants), functions and operators starting with it, those the document uses most
  first; ↑/↓ choose, Enter, Tab or a click take one (a function as its call), typing
  on ignores it.
  Matrices (`mat`): `,` goes to the next cell of the row or adds one at its end,
  `;` or Enter add a row below, ↑/↓ move between rows keeping the column,
  Backspace in an empty cell that starts a row joins it to the row above.
  Options of math calls are edited in a tab above the call. Code in a formula
  (`#sym.brace.l`) is walked through by ←/→ and edited in place, its source showing
  while the caret is in it; Esc leaves it.
- **Structure**: ←/→ walk all positions, entering and leaving structures;
  environments have positions just before and after them; footnote text is reached
  after its paragraph, where it is shown. Backspace on a structure selects it first,
  then deletes; at the start of a body it marks the call, then unwraps it. Esc leaves
  the innermost structure.
- **Lists**: `- `, `+ `, `/ ` start items; Enter, Backspace, Tab, Shift+Tab as in word
  processors.
- **Labels and references**: labels are pills beside the page, entered only by
  clicking. `@` starts a reference with completion of labels and bibliography keys;
  Ctrl+click follows it, Alt+← goes back. Jumps land a third down the view.
- **Selection**: Shift+moves, Shift+click, drag; normalized to whole pieces (whole
  blocks across blocks). Copy writes Typst as the export does; paste reads Typst.

## Files

- **`.sem`**: a zip holding `document.json` (header, blocks), `remarks.json`,
  `main.typ` (the export, not read back), the project's files (pictures, `refs.bib`),
  `scratch/` (kept, never versioned or exported) and a git repository under `.git/`.
- **History**: every save that changes something is a commit; versions can be named
  (tags), opened read-only and restored.
- **Export**: a Typst project folder (`main.typ` and its files) or a PDF.
- **Import**: `.typ` files open as untitled documents (one way): the leading
  definitions become the header, the rest is mapped to the tree, what is left stays
  raw; local files it names are copied in.

## App

- Left sidebar: Outline (headings rendered by Typst) and Versions.
- Right sidebar: Remarks (anchored to blocks or ranges, a range also across blocks
  such as a display formula; following edits; kept out of the Typst; a card each on
  old-paper yellow, on top a read-only quotation cut from the paper's render (a
  range's lines with one line above and below; a block's remark the whole block),
  its text typed right in the card with the paper's editor (formulas, markup, the
  paper's own functions, `@label` references to the paper's sections, equations
  and environments, offered while typing and shown with the paper's numbers; kept
  as Typst), a click in the text edits it, no author, date or replies; an empty one
  goes when left) and Chat with Claude (through the user's own Claude Code, `claude -p`;
  answers typeset by Typst; not saved).
- Problems: Next Problem visits blocks that fail, broken references and duplicate
  labels.
- Dialogs edit the header, code, diagrams, the bibliography and the title fields,
  with a live preview; Typst source is highlighted (markup, or code alone in
  diagrams and code in formulas).

## Known limits

- Anything that needs the whole document at once (outline content, last page,
  forward `query`) is approximate in the editor; the export is a normal compile.
- A header template that produces content shows it in every block.
- Code in a formula is boxed in the editor so the caret finds it; a box loses limits
  and `lr` scaling, so such operators may look different from the export.
- A restated block with a footnote shows the footnote twice in the editor.
- Labels made in code (`label("…")`) are not seen; references to them are reported.
