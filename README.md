# Semantic Editor

**A structural editor for mathematical writing, built on [Typst](https://typst.app).**
You type on the typeset page itself. There is no source pane next to a preview.
Formulas are trees: the caret moves into fractions, scripts and function arguments, as in
[TeXmacs](https://www.texmacs.org) or [LyX](https://www.lyx.org). What you see is
Typst's own output, and the document saves and exports as ordinary Typst.

![The editor with the sample document: outline pane, numbered headings, an equation, a
CeTZ diagram in a figure, label pills beside the page](docs/screenshots/editor.png)

> **Status:** an early prototype. Expect rough edges, and keep backups of anything
> important.

It is a native GNOME app, written in Rust with GTK 4 and libadwaita. Typst 0.15 is built in.

## Why

Typst source is pleasant to write, but math-heavy text in source form is still hard to
read while you are writing it. WYSIWYG math editors usually have their own typesetting
and their own file formats. This editor tries to give you both:

- **Edit the result.** Every block of the document is compiled by Typst and drawn as it
  will print. The caret lives in that picture.
- **Structure, not characters.** `a/b` makes a fraction with the caret in the
  denominator. `^` and `_` make scripts. ← and → step into and out of structures, and
  Backspace on a structure selects it first, then deletes it.
- **Plain Typst underneath.** Export gives an ordinary Typst project, and existing
  `.typ` files open as editable documents. Whatever the editor does not understand stays
  as raw Typst that you can still edit.
- **Fast.** Only the blocks you change are recompiled, and renders are cached in memory
  and on disk.

## Features

### Writing text

- Paragraphs, headings (Ctrl+1 … Ctrl+6, Ctrl+0 for a paragraph), *emphasis* and
  **strong** with `_…_` and `*…*`, inline `code`, links, non-breaking spaces, dashes and
  line breaks.
- Any Typst call with a content body, inline (`#underline[…]`, your own `#note[…]`) or as
  a block (`#align(center)[…]`). Backspace at the start of a call unwraps it.
- Bullet, numbered and term lists, nested with Tab and Shift+Tab.
- Theorem-like environments from your own header functions (`#lemma[…]`, `#proof[…]`),
  with optional titles, nesting and numbering.
- Footnotes, numbered through the document and shown under their paragraph.

### Formulas

- Inline `$…$` and display `$$` formulas. Display formulas can be numbered and span
  several lines, with alignment points `&`.
- Fractions, scripts, primes, roots, accents, `cases`, function calls with their
  arguments, `lr(…)` with a size, and text in formulas.
- Named options of a call (`size: 80%`) go in a small tab on the call's outline,
  reached with ↑ or a click. They never push the formula around.
- Symbol shortcuts: `->`, `<=`, `alpha`, `\alpha`, … become →, ≤, α.
- CeTZ diagrams, both as blocks and inside formulas, edited in a source dialog with a
  live preview.

### Figures, labels and references

- **Pictures:** *Insert Picture…* copies a PNG, JPEG, SVG, … file into the project and
  makes a figure of it. Type the caption under it, and set the width in the picture's
  options tab.
- Figures with captions, around pictures, diagrams or anything else.
- Labels on headings, equations, figures and theorems, shown as pills beside the page.
  A click on a pill edits the label.
- References typed with `@`, with a completion list of the labels. Ctrl+click follows a
  reference, and Alt+← goes back.
- A BibTeX bibliography and citations, numbered as in the full document.

### Navigating and checking

- An **outline pane** with every heading as Typst draws it, numbers and formulas
  included. A click jumps to a heading.
- A breadcrumb of where the caret is (*paragraph › math › fraction › numerator*) and a
  hint line with the keys that apply there.
- *Next Problem* (F8) goes through failed compiles, references to missing labels and
  duplicate labels.
- Selection with Shift and the arrow keys, Shift+click or dragging. Copy, cut and paste
  use Typst source, so text from any `.typ` file can be pasted.
- Undo and redo of everything. Zoom with Ctrl+plus and Ctrl+minus.
- **Remarks** (Ctrl+Alt+M): a note on the selected text, or on the whole block the caret
  is in (a theorem). They are kept beside the text, not in it: the selected text is
  shaded yellow, every remark gets a yellow bar in the margin, and pointing at
  either shows the remark. Remarks follow the text as you edit it. The Remarks sidebar
  on the right lists them in the order of the text: click one to select its text, reply
  to it, resolve it; the remarks the caret is in are highlighted there.
- **Chat with Claude:** the Chat tab beside Remarks asks Claude about the
  paper through your logged-in [Claude Code](https://claude.com/claude-code) (`claude`
  on the host; no API key). Claude sees the whole paper as Typst; with text selected,
  the question quotes it. Answers are typeset by Typst, formulas and the paper's own
  notation included; *Copy* puts an answer's Typst on the clipboard. Claude cannot
  change the paper. Conversations are throw-away: not saved, gone with the window or
  when another document is opened.

### Files

- **Projects:** a `.sem` file is a zip of the document, a `main.typ` generated on every
  save, and the project's files: `images/…` and `refs.bib`. Unzipped, it compiles with
  the `typst` command line as is.
- **Opening `.typ` files:** your existing Typst document becomes editable blocks, and
  the pictures and `.bib` files it uses are copied in. Header definitions, show rules and
  code are kept.
- **Version history:** every save is a version, kept in a git repository inside the
  `.sem` file. The *Versions* tab lists the saves by day, and you can give versions
  names such as "arXiv submission". An old version opens read-only in its own window,
  and *Restore this Version* brings it back as a new version, so nothing is lost.
- **Scratch files:** scans of handwritten notes and draft figures can be inserted as
  scratch pictures. They are kept with the document but stay out of its versions and
  exports, until you choose *Keep*.
- **Export:** a PDF, or a Typst project written into a folder.
- **The document header** holds your `#set`, `#show` and `#let` lines and is edited in a
  dialog. A title block holds the title, authors and date.

## Keys

| Key | Effect |
| --- | --- |
| `$`, `$$` | inline formula, display formula |
| `/`, `^`, `_` | fraction, superscript, subscript of what is before the caret (or of the selection) |
| ← → | move by one place, entering and leaving structures |
| ↑ ↓ | numerator ↔ denominator, base ↔ scripts, between lines; ↑ into an options tab |
| Esc | out of the structure the caret is in |
| Tab, Shift+Tab | next or previous empty slot; indent or outdent a list item |
| Backspace | after a structure: select it, then delete it |
| Enter, Shift+Enter | new paragraph (or list item, or case), line break |
| Ctrl+Enter or double click | edit a diagram's or raw block's source |
| Ctrl+click, Alt+← | follow a reference, go back |
| F8, Shift+F8 | next or previous problem |
| Ctrl+Alt+M | add a remark on the selection or the block |
| Ctrl+Z, Ctrl+Shift+Z | undo, redo |
| Ctrl+C, Ctrl+X, Ctrl+V | copy, cut, paste (as Typst) |
| Ctrl+S, Ctrl+O, Ctrl+E | save, open, export PDF |

## Planned

- Matrices and tables.
- Selecting precisely across paragraphs, highlighting matching brackets, and putting
  `lr(…)` around a selection.
- A keyboard way into labels and options tabs (only a click today).
- Comparing versions, parallel versions (branches), and saving a copy without its
  history.
- Multi-file Typst projects (`#include`), and files named by included files.
- Editing the arguments of an existing call, and support for more Typst packages.
- Sharp zooming (vector rendering) and Typst syntax highlighting in the source dialog.

What the editor should do is in [`spec.md`](spec.md), how it is built and why in
[`NOTES.md`](NOTES.md), and what is planned in [`TODO.md`](TODO.md). Bugs and
requests: GitHub issues.

## Building

The app is built with Flatpak against the GNOME 51 SDK. You need `flatpak`,
`flatpak-builder`, the `org.gnome.Sdk//51` runtime and the
`org.freedesktop.Sdk.Extension.rust-stable` extension (both from Flathub).

```sh
git clone https://github.com/molnarandris/semantic-editor.git
cd semantic-editor

# build, install and run
flatpak-builder --user --install --force-clean build-dir \
    build-aux/io.github.molnarandris.SemanticEditor.Devel.json
flatpak run io.github.molnarandris.SemanticEditor.Devel

# tests and lints, inside the SDK
build-aux/sdk cargo test
build-aux/sdk cargo clippy --all-targets
```

GNOME Builder opens the project with the same manifest. The build needs network access
(Rust crates). CeTZ is downloaded from Typst Universe the first time a diagram is shown.
Try *Open Sample Document* in the menu for a tour of the features.

## How it is made

| Crate | Role |
| --- | --- |
| `model` | the document tree, caret paths, editing operations, undo. No GTK, no Typst |
| `typst-bridge` | Typst source to and from the tree, the compile worker, render caches, mapping layout back to nodes |
| `editor-widget` | the `SemEditor` GTK widget: drawing, caret, keys, mouse |
| `app` | window, outline pane, dialogs, files, export |

Each block compiles as its own small Typst document, with the header, the definitions
before it and the counter values it starts with. The widget draws the result and finds
caret positions from the layout. The document model, not the Typst source, is the source
of truth.

Most of the code was written with [Claude Code](https://claude.com/claude-code), following
the specification in `spec.md` and the notes in `NOTES.md`.

## License

GPL-3.0-or-later: see [`LICENSE`](LICENSE). You may use, study, share and change the
program. If you distribute it, or a changed version, you must do so under the same
license, with its source.
