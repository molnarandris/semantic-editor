# To do

Where the work stands. Delete items when done (git keeps the history). Bugs belong
in GitHub issues; the ones below were found before that and are to be moved there.

## Waiting for the user's test

- Code pieces that fail show their source on red (inline pieces, inline calls,
  block calls, code in formulas); a name defined nowhere typed with a space is
  code. Code in formulas is edited in place (←/→ into it, Esc out).
- The cleanup of 2026-11-16 (widget split into modules, `jobs::Targets`): no
  behaviour change intended; worth a general look around.
- Not confirmed yet, from earlier rounds: remarks sidebar (R2–R3), chat (C1–C2),
  version history UI, projects and pictures, labels/outline/footnote navigation,
  selection/copy/paste decisions.

## Next

From the user's list of 2026-11-15, one group at a time:

1. A way to turn greedy line breaking off.
2. Syntax highlighting in the header, code and figure editors (GtkSourceView with
   Typesetter's Typst language file, GPL).
3. Formulas: typing `#` in a formula to start code (code atoms come from import
   and paste); text with visible bounds that can turn back into code (idea: only `"`
   starts text; anything else is Typst, read as code on space); the header's
   functions (`bigbx`) cannot be typed (offer `times.big` / box symbols?); copy and
   paste of a formula holding `#bigbx` breaks subscripts; symbol completion while
   typing a name, most used first; matrices.
4. Remarks rework: a selection across a display formula remarks only the text before
   it; remark text in the editor's syntax, rendered by Typst; no prominent author,
   date or replies; the card shows the remarked text rendered; maybe in the margin
   instead of a sidebar (undecided).
5. Chat: clickable references to the paper's parts; Claude can add remarks.
6. Sidebar buttons look bad (postponed by the user, not important).

Then the planned features:

- **S — Suggestions**: a range and its replacement kept outside the text; Accept
  (an ordinary, undoable edit) and Reject; Claude's chat can leave them.
- **D — Comparing versions**: two versions read-only with insertions and deletions
  marked (blocks matched by id, text by word, formulas whole), a diff PDF;
  differences turned into suggestions.

## Open questions for the user

- A keyboard way into a label pill and an options tab (today only a click or ↑).
- Remarks beside the page instead of a sidebar?

## Wishlist

- Branches (parallel versions), saving a copy without history.
- Footnote texts listed at the end, the paragraph showing only the number.
- Matrices and tables (2D arguments); `lr` around a selection; precise
  cross-paragraph selections; Ctrl+B/I on a selection.
- Editing a call's arguments after typing them; functions from packages.
- Explicit item numbers and wide lists on import; numbered references to raw
  environments (custom figure kinds); files named by an included `.typ` or paths
  built in code.
- Keywords, abstract, a date on the page, affiliations in the title block.
- Citation forms other than `@key`; showing non-breaking spaces; `-?`, `...`.
- Headings inside environments in the outline; remember whether panes are shown.
- Draggable sidebar width (libadwaita split views cannot).

## Known bugs (move to GitHub issues)

- `insert_block` (diagram, raw Typst) from a figure caption probably puts the block
  inside the figure, after the caption.
- A reference still being typed when the file is saved keeps a trailing `.`/`:`.
- Joining two paragraphs (Backspace at the start of the second) detaches a remark on
  the second's text until undo.
- After a theme switch, outline entries re-render only when their heading compiles
  again.
