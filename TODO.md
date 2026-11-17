# To do

Where the work stands. Delete items when done (git keeps the history). Bugs belong
in GitHub issues; the ones below were found before that and are to be moved there.
Items that are Claude's own ideas, not the user's requests, are marked *(idea)*.

## Waiting for the user's test

- Remarks across blocks: a selection over a display formula remarks all of it,
  from where it starts to where it ends (shaded across the formula).

## Next

From the user's list of 2026-11-15, one group at a time:

1. Remarks rework (user decisions 2026-11-17): stay in the sidebar for now (maybe
   later between paragraphs); cards on faint yellow (old paper); on top a read-only
   quotation of the remarked text, for a word or phrase its line plus one above and
   one below (a whole paragraph for a paragraph's remark is ok for now); the remark
   text edited like the paper, by the same editor widget for one paragraph; no
   author or date at all (user 2026-11-17), no replies.
2. Chat: clickable references to the paper's parts; Claude can add remarks.
3. Sidebar buttons look bad (postponed by the user, not important).

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
- Tables (2D arguments); `lr` around a selection; precise
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
