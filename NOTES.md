# Notes for developers

How the editor is built and why: decisions, pitfalls and facts about Typst that were
measured, not guessed. The specification is [spec.md](spec.md); history is in git (the
commit messages say what changed and why).

## Versions and build

- Typst **0.15.1** (`typst`, `typst-layout`, `typst-render`, `typst-pdf`, `typst-kit`),
  gtk4-rs 0.11 (`v4_18`), libadwaita 0.9 (`v1_8`), sourceview5 0.11, CeTZ 0.5.3
  (`cetz_version!`). GNOME Platform/SDK 51 with `rust-stable//26.08`.
- App ID `io.github.molnarandris.SemanticEditor` (`.Devel` for dev builds), baked in
  from `APP_ID` at compile time. Manifest:
  `build-aux/io.github.molnarandris.SemanticEditor.Devel.json`.
- Flatpak only, no distribution `-devel` packages. CLI work runs inside the SDK:
  `build-aux/sdk cargo test|clippy|run` (target dir `target/sdk`). Install with
  `flatpak-builder --user --install --force-clean build-dir build-aux/*.Devel.json`.
- GNOME Builder's play button ignores `build-commands` and runs its own `cargo build`
  in a sandbox where `~/.cargo` is an empty tmpfs, hence `CARGO_HOME=target/cargo-home`
  in the manifest env. Cargo needs every crate of `Cargo.lock` cached or it goes to
  the network; after dependency changes run once online:
  `build-aux/sdk sh -c 'CARGO_HOME=target/cargo-home cargo fetch'`.
- Flathub would need `cargo-sources.json` (`flatpak-cargo-generator`); the build uses
  the network for now. The app has network access for Typst packages.
- Tests that compile CeTZ download the package on their first run.
- `build-aux/sdk` passes the Wayland socket so tests can initialise GTK (the
  highlighting test); no window opens.
- Highlighting: `data/languages/typst.lang` is Typesetter's (GPL-3.0-or-later,
  copied unchanged, only `def:` styles, so any scheme works); `typst-code.lang` is
  ours, its `code` context alone, for diagrams. Both are installed in
  `/app/share/gtksourceview-5/language-specs`, which the default language manager
  searches; `build-aux/sdk cargo run` has no highlighting (nothing installed there).

## Fonts and preamble

- Only the fonts embedded in `typst-kit` (New Computer Modern + Math, Libertinus,
  DejaVu Sans Mono): deterministic, no fontconfig. `today()` returns `None`.
- The editor's preamble (fonts, `sem-slot`, `sem-code`) comes before the header, so a
  header can override the font. New documents get `#set heading(numbering: "1.1")`.

## Rendering pipeline

- `block_document` wraps a block: preamble, header + code blocks before it, the
  editor's `#set page(width, height: auto, margin: 8pt, …)` (after the header, so the
  header's paper and page decorations do not apply), prefix, snippet, suffix.
  `BLOCK_PAD_PT` keeps descenders and big operators inside the raster; layout
  coordinates are relative to the content box.
- The worker caches `RawLayout` (geometry by snippet byte offsets, no node ids);
  `resolve(&SpanMap)` maps it to nodes per job, so blocks with identical source
  share a render. A block is stale when its snippet *or its span map* changed (same
  text with new ids would leave a layout pointing at dead ids).
- `LAYOUT_VERSION` is in the cache key: bump it when layout extraction changes.
- Results reach the GTK main loop via `glib::SendWeakRef` + `MainContext::invoke`.
- Counter chain (`counters.rs`, `jobs::stale_blocks_editing`): a sensitive block is
  submitted only once the previous sensitive block's end state is known; a changed
  end state ripples down one compile at a time, an unchanged one stops it. A failed
  block passes its start state on. Forced re-renders clear the views' submitted
  snippets rather than passing a flag.
- Failed pieces: errors come back with their place (`compile_located`, the error's
  span then its trace); the innermost node there is blamed if it is a code piece or
  call (`jobs::failing_source`) and the block is submitted again with it shown as
  source. A failed piece stays so while its source is unchanged.
- Preview, outline-entry and snippet (chat answer) jobs share the worker with
  reserved job ids (`PREVIEW_ID`, `OUTLINE_BIT`, `SNIPPET_BIT`) and are `transient`
  (not cached).

## Pitfall: sharp rendering

Learned the hard way (blurry text that Typesetter did not have):

- Draw rasters with `snapshot.append_texture`, never `append_scaled_texture`: GTK's
  scaled-texture node resamples even at exactly 1:1 (GL and Vulkan alike).
- Render at zoom × device scale and draw the texture at its own size in device
  pixels, at a whole-device-pixel position. Any mismatch resamples.
- Not causes (measured): the font, anti-aliasing settings, debug vs release,
  transparent block pages.
- Debug by dumping the raster as PNG and comparing with a magnified screenshot; render
  offscreen with `Gsk.*Renderer.realize_for_display` + `render_texture` (PyGObject in
  the SDK) to see what GTK does. When matching Typesetter is an option, do that.

## Spans and the caret

- Glyph spans were fine enough for math; no marker boxes were needed in general.
- `sem-slot` and `sem-code` are `box.with(…)`, so each frame carries its call site's
  span (a closure would give them the preamble's).
- Generated text (an equation number, a heading number) has the element's span with
  a delta counting its own chars; a delta only counts for spans of leaf syntax nodes
  (`layout.rs`, `span_info`), else numbers leak onto the formula's first node.
- What a function from the header draws carries the header's spans. Calls of such
  functions are written `class("normal", #sem-code()name(args)#sem-code())` in the
  editor so the empty boxes carry the call's span on both sides.
- An empty paragraph, heading, title or call body shows a transparent `x` in the
  editor, so the caret has a glyph and a line. A zero-width space does not work
  (Typst drops it when shaping).
- Caret stops (`caret.rs`) are pure geometry, testable without GTK; cached per block
  view and reset when the block is resubmitted or a render arrives.

## Typst facts (measured)

- Math spacing: a written space shows only next to a "spaced" item (fence `|`, text,
  box), not at a group's edge, after a line break, or where an operator brings its
  own spacing. So `|x|` ≠ `bar.v x bar.v`, `"Rep" (A)` ≠ `"Rep"(A)`, and a space next
  to code (a box) in a formula shows.
- Code after `#` in a formula does not see the math module: `#dif` fails,
  `#math.dif` works (`symbols::math_only`); the header's names are seen.
- `f'` attaches a prime, `f '` does not; after a fraction a prime would attach to the
  denominator.
- `a*b*c` and `snake_case` are plain text; `a@b.c` is a reference; `word#footnote[…]`
  is a call.
- A `;` right after an expression in markup ends it and vanishes; `#x;` is needed
  before `.`, `(`, `[` or text that would continue the expression.
- A string in a formula keeps every space; markup (`#[…]`, which the editor uses
  to map each char of a formula's text) collapses runs and drops them at its ends.
  So the editor writes the first space after a word as markup and every other one
  as `#" ";`: same layout, but split glyph runs anti-alias a pixel differently.
- `liminf` contains a U+2009 thin space in Typst's text.
- A blank line between a paragraph and its list adds space; a block call on lines of
  its own ends the paragraph above it.
- An escaped bracket in math (`\{`) does not pair and stretch; the tree's brackets
  do, so imports keep it as `#sym.brace.l`.
- In a block page of auto height, footnotes go under the paragraph after a separator.
- Footnotes inside `hide`, `place` or a clipped box are still printed, whatever the
  show rules (why a restated block with a footnote shows it twice).
- `par(justify: true)` breaks lines optimally, which may move every line at each
  keystroke; `linebreaks: "simple"` leaves the lines above an edit alone.
- `chevron.r` exists in 0.15; `angle.r` is gone.

## Decisions (and why)

- **Per-block compiles with chained state**, not whole-document compiles: typing
  costs one small compile; counters, references and citations are made right by the
  prefix/suffix machinery above.
- **Function shapes by parsing, not compiling** (`calls.rs`): it runs on every
  keystroke. A name bound in a way the parser cannot read (`#let thm = thmbox(…)`, an
  import, `: *`) may take anything; a name defined nowhere is code.
- **Widget-drawn decorations** (label pills, reference tint, `&` marks, outlines,
  options tabs, completion list) instead of Typst output: no recompile when the caret
  moves, and nothing changes the typeset spacing (a zero-width box at `&` changes the
  spacing around `=`).
- **No GTK popover for completion**: the editor keeps focus and the input method.
- **Bibliography** written as `refs.bib` in exports and the project, but embedded in
  the editor's own renders; the block keeps its source for undo.
- **One-way import**: a `.typ` is never written back.
- **History in git inside the zip** (libgit2 bundled): one pack, packed refs; author
  is the account's real name with the placeholder email `none`.
- **Chat runs the user's Claude Code** (`claude -p`, stream JSON, no tools, no
  session persistence) through `flatpak-spawn --host`; `--bare` cannot use the
  subscription. Stateless: the paper and the conversation go on stdin each time.
- **No backslash commands in math**: names resolve when the word ends, as in Typst.
- **Remark quotations** are cut from the blocks' textures (`imp/quote.rs`), not
  rendered again, so the lines are the paper's; a key of the blocks' render
  generations and the cut tells the sidebar when to redo one. Screenshots under
  Broadway fail while the remarks sidebar opens at startup (an AdwBin gets a garbage
  width; not seen on Wayland).
- **Matrix rows**: `MathKind::Call.rows` holds where each `;` row starts. Typst ends
  embedded code at a direct `;` and swallows it (`#sem-slot();`), so the serializer
  puts a space before `;` after a cell that may end in code (an empty cell too).

## Tools

- `crates/typst-bridge/src/bin/bench.rs`: cold and warm compile times.
- `crates/typst-bridge/examples/import_report.rs`: imports a `.typ`, lists raw blocks;
  `--compare` renders original and round trip and names differing pages (output in
  `target/import-check/`; the user's paper is never committed).
- Screenshots without a window on screen (GTK Broadway inside the SDK, private bus):

  ```sh
  build-aux/sdk dbus-run-session -- sh -c 'gtk4-broadwayd :9 & B=$!; sleep 1;
    GDK_BACKEND=broadway BROADWAY_DISPLAY=:9 SEM_SAMPLE=1 SEM_SCREENSHOT_DELAY=10000 \
    SEM_SCREENSHOT=docs/screenshots/editor.png target/sdk/debug/semantic-editor; kill $B'
  ```

  Use a display number not used before. `SEM_SAMPLE=1` opens the sample document.
- The sample document (`app/src/sample.rs`) shows every feature, partly typed key by
  key through `Editor`; `sample_has_every_feature` checks its export.

## Testing

- Every change: `build-aux/sdk cargo test --workspace`, `cargo clippy --all-targets`
  clean, `cargo fmt`.
- Serializer: golden strings, every typing state compiles, edit and readable styles
  render the same pixels. Import: import then export renders the same pages.
- Model: op/inverse round trips, a keystroke fuzz test (`model/tests/random.rs`).
- Widget-only behaviour (drawing, dialogs) has no tests; it is checked by hand.
