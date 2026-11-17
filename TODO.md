# To do

Where the work stands. Delete items when done (git keeps the history). Bugs belong
in GitHub issues; the ones below were found before that and are to be moved there.
Items that are Claude's own ideas, not the user's requests, are marked *(idea)*.

## Waiting for the user's test

- Remark text edited in its card by the paper's editor (formulas, markup, the
  paper's own functions), kept as Typst; cards stay cream with dark text in a dark
  theme (the quotation is black, as on the paper).
- A click in a card's text edits it (it needed a double click).
- "I want to reference sections, equations etc of the original doc from within the
  remark": `@` in a card offers the paper's labels and shows the paper's numbers;
  Ctrl+click on one goes there in the paper.
- The completion list (references, names in formulas) is a real pop-up, no longer
  cut to a remark's card ("The reference completion pop-up is not visible on the
  remark card as it gets truncated"); typing, ↑/↓, Enter/Tab and clicking a row
  should work as before, in the paper too.

- A card's quotation at the size of the card's text, whatever the window, the zoom
  or the screen; shrunk only when the paper's lines are wider than the card ("If I
  make the window larger, the size of the font seems to shrink").
- The remarked range shaded in its card's quotation, as in the paper ("when it is
  short and lines above and below were included in the quote, I want the highlight
  to show up in the remark card as well").

- Chat: "clickable references to the paper's parts": Claude writes `@label` for
  labelled parts; the answer shows it as the paper does, with its number, and a
  click goes there. Answers are now shown by a read-only editor (selectable,
  re-flowing with the sidebar).
- The chat no longer quotes the selection a remark leaves in the paper ("my last
  remark's highlight is passed to the chat").

## Known limits

- A card's quotation still shrinks in a wide window: it is as wide as the paper's
  text column, empty part included. Re-rendering it at the card's width was
  considered and postponed by the user (needs its own jobs on the shared compile
  thread).

## Next

From the user's list of 2026-11-15, one group at a time:

1. Chat: Claude can add remarks (clickable references done, above).
2. Sidebar buttons look bad (postponed by the user, not important).

Then the planned features:

- **S — Suggestions**: a range and its replacement kept outside the text; Accept
  (an ordinary, undoable edit) and Reject; Claude's chat can leave them.
- **D — Comparing versions**: two versions read-only with insertions and deletions
  marked (blocks matched by id, text by word, formulas whole), a diff PDF;
  differences turned into suggestions.

## Open questions for the user

- A keyboard way into a label pill and an options tab (today only a click or ↑).

## Wishlist

- Remark cards between the paragraphs instead of the sidebar (user, maybe later).
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
