# To do

Where the work stands. Delete items when done (git keeps the history). Bugs belong
in GitHub issues; the ones below were found before that and are to be moved there.
Items that are Claude's own ideas, not the user's requests, are marked *(idea)*.

## Waiting for the user's test

- Copy and paste of formulas with scripts on code or groups (`attach`).
- Formulas: unknown words are code (red when failing), text only with `"`; code
  and text edited in place and outlined while the caret is in them.
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

1. Formulas (in progress, user decision 2026-11-16): text only with `"` (done:
   other words are code); text and code edited in place, typing anywhere in them
   (done); outlined while the caret is in them, like calls (done); copy and paste
   of a formula holding `#bigbx` (done). Then: symbol completion while typing a
   name, most used first; matrices ("almost there, not much needed").
2. Remarks rework: a selection across a display formula remarks only the text before
   it; remark text in the editor's syntax, rendered by Typst; no prominent author,
   date or replies; the card shows the remarked text rendered; maybe in the margin
   instead of a sidebar (undecided).
3. Chat: clickable references to the paper's parts; Claude can add remarks.
4. Sidebar buttons look bad (postponed by the user, not important).

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
