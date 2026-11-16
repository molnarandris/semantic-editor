//! Tree → Typst source. The tree is the source of truth; Typst source is
//! always derived. Each block serializes to a standalone snippet plus a
//! [`SpanMap`] from node ids to byte ranges in that snippet.

use std::ops::Range;

use typst::syntax::ast::{self, AstNode};

use model::{
    Block, BlockNode, Document, Fragment, Inline, InlineNode, ListKind, MathKind, MathNode,
    MathRow, NodeId, OPERATORS, TitleFields, spaced_gap,
};

/// Shared definitions and styles. Prepended to every block document and to
/// the exported `.typ` file.
///
/// `sem-slot` must stay a `box.with(..)`: then the box element is created at
/// the call site and its frame carries the slot's own span. A closure
/// (`#let sem-slot() = box(..)`) would give every slot the preamble's span.
/// `sem-code` wraps code in a formula in the editor's renders: its
/// transparent stroke carries the code's span, so the code gets a box even
/// when what it draws comes from a package (CeTZ shapes carry CeTZ's spans).
pub const PREAMBLE: &str = "\
#set text(font: \"New Computer Modern\", size: 11pt)
#show math.equation: set text(font: \"New Computer Modern Math\")
#let sem-slot = box.with(width: 0.55em, height: 0.7em, stroke: (paint: luma(160), thickness: 0.5pt, dash: \"densely-dashed\"))
#let sem-code = box.with(stroke: 0.5pt + rgb(0, 0, 0, 0))
";

/// The header of new documents.
pub const DEFAULT_HEADER: &str = "#set heading(numbering: \"1.1\")\n";

/// What the header dialog previews: a little of everything a header styles.
pub const HEADER_SPECIMEN: &str = "\
= Section heading
Body text with inline math $x^2 + y^2 = r^2$, set in the document's font and size.

== Subsection heading
$ integral_0^1 f(x) dif x = F(1) - F(0) $";

macro_rules! cetz_version {
    () => {
        "0.5.3"
    };
}

/// The CeTZ version diagrams are drawn with. Pinned so that documents do not
/// change when CeTZ releases a new version.
pub const CETZ_VERSION: &str = cetz_version!();

/// The import that diagram blocks need.
pub const CETZ_IMPORT: &str = concat!("#import \"@preview/cetz:", cetz_version!(), "\"");

/// Opens the code font of a call being typed (closed by `]`).
const CODE_TEXT: &str = "#text(font: \"DejaVu Sans Mono\", size: 0.9em, fill: rgb(\"#6b6390\"))[";

/// Opens content on a red background: code that failed to compile.
const FAILED: &str = "#highlight(fill: rgb(\"#f7c6c6\"))[";

/// A call's head in markup: `#name`, or `#name(args)` with arguments.
fn call_head(name: &str, args: &str) -> String {
    if args.is_empty() {
        format!("#{name}")
    } else {
        format!("#{name}({args})")
    }
}

/// `source` (a call's head or end) in the code font on red, unmapped.
fn failed_source(source: &str) -> String {
    let escaped = source.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{FAILED}{CODE_TEXT}#\"{escaped}\"]]")
}

/// The calls being typed in a text, as ranges of char indices: `#`,
/// `#name` and `#name(…` (to its `)`, or to the end), anywhere in it, as
/// typing space, `[` or `(` turns them into calls (`Editor::insert_text`).
/// A lone `#` only at the end (where it is being typed); `#1`, `# x` and
/// `#(x` are none.
fn calls_being_typed(chars: &[char]) -> Vec<(usize, usize)> {
    let is_name = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let mut runs = vec![];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '#' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && is_name(chars[j]) {
            j += 1;
        }
        let name = &chars[i + 1..j];
        let ok = match name.first() {
            None => j == chars.len(),
            Some(&c) => c.is_alphabetic() || c == '_',
        };
        if !ok {
            i = j.max(i + 1);
            continue;
        }
        if !name.is_empty() && chars.get(j) == Some(&'(') {
            let mut depth = 0;
            while j < chars.len() {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                j += 1;
                if depth == 0 {
                    break;
                }
            }
        }
        runs.push((i, j));
        i = j;
    }
    runs
}

/// Inlines holding only an empty text.
fn is_empty_text(inlines: &[InlineNode]) -> bool {
    matches!(inlines, [InlineNode { kind: Inline::Text(s), .. }] if s.is_empty())
}

/// The Typst source of an empty math row (a visible placeholder box).
pub const SLOT: &str = "#sem-slot()";

/// Maps node ids to byte ranges of the snippet they were serialized into.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanMap {
    /// In serialization (pre-)order: parents before children.
    entries: Vec<(NodeId, Range<usize>)>,
    /// For each text inline: the byte offset of every char, plus the end.
    texts: Vec<(NodeId, Vec<usize>)>,
}

impl SpanMap {
    pub fn range(&self, id: NodeId) -> Option<Range<usize>> {
        self.entries
            .iter()
            .find(|(n, _)| *n == id)
            .map(|(_, r)| r.clone())
    }

    /// The innermost node whose range contains `offset`. On ties (a row
    /// and its single node) the deeper node wins.
    pub fn innermost(&self, offset: usize) -> Option<NodeId> {
        self.entries
            .iter()
            .rev()
            .filter(|(_, r)| r.contains(&offset))
            .min_by_key(|(_, r)| r.len())
            .map(|(n, _)| *n)
    }

    /// All `(id, range)` entries, parents before children.
    pub fn entries(&self) -> &[(NodeId, Range<usize>)] {
        &self.entries
    }

    /// The char index within text inline `id` at source byte `offset`
    /// (escapes count as one char).
    pub fn text_char_at(&self, id: NodeId, offset: usize) -> Option<usize> {
        let (_, starts) = self.texts.iter().find(|(n, _)| *n == id)?;
        let first = *starts.first()?;
        let last = *starts.last()?;
        if offset < first || offset > last {
            return None;
        }
        Some(starts.partition_point(|&s| s <= offset).saturating_sub(1))
    }

    /// Source byte offset of char `index` in text inline `id`.
    pub fn text_byte_of(&self, id: NodeId, index: usize) -> Option<usize> {
        let (_, starts) = self.texts.iter().find(|(n, _)| *n == id)?;
        starts.get(index).copied()
    }

    /// Shift every range by `delta` bytes (e.g. after prepending a preamble).
    pub fn shift(&mut self, delta: usize) {
        for (_, r) in &mut self.entries {
            *r = r.start + delta..r.end + delta;
        }
        for (_, starts) in &mut self.texts {
            starts.iter_mut().for_each(|s| *s += delta);
        }
    }
}

/// How the Typst source is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// For the editor's own renders: every glyph must carry a span inside
    /// the node it belongs to, so groups are always parenthesized and
    /// operators spelled `op(#[log])`.
    Edit,
    /// For people (export): brackets only where Typst needs
    /// them, plain operator names, literal punctuation. Renders the same.
    Readable,
}

struct Writer {
    out: String,
    map: SpanMap,
    style: Style,
    /// Depth of function-call arguments (`attach(…)`), where a literal comma
    /// would end the argument.
    args: usize,
    /// Written after every line break: list item bodies are indented.
    indent: String,
    /// The number of a numbered item rendered on its own (Edit style), so
    /// it shows its place in the list: `3. …` instead of `+ …`.
    number: Option<usize>,
    /// The markup delimiters (`*`, `_`) the writer is inside of.
    markup: Vec<char>,
    /// The code piece in text whose source shows (Edit style): the caret is
    /// in it.
    open: Option<NodeId>,
    /// Code pieces and calls that failed to compile (Edit style): their
    /// source shows, on red.
    failed: Vec<NodeId>,
}

/// Whether inline call `i` of `inlines` (bold or italic) can be written as
/// markup, `*…*` or `_…_`, and read back the same by Typst: the delimiters
/// must not touch a letter or digit (Typst reads those as plain text), the
/// body must not start or end with a space, and nothing else may make the
/// delimiter ambiguous (an empty text next to another call or formula). A
/// call inside one written the same way is written as a call.
fn markup_ok(inlines: &[InlineNode], i: usize, m: char) -> bool {
    let text = |k: usize| match inlines.get(k).map(|n| &n.kind) {
        Some(Inline::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    let (Some(before), Some(after)) = (text(i - 1), text(i + 1)) else {
        return false;
    };
    let Some(Inline::Call { body, .. }) = inlines.get(i).map(|n| &n.kind) else {
        return false;
    };
    // The first and last char the body is written with (`$` or `#` for a
    // formula or call).
    let edge = |n: &InlineNode, end: bool| match &n.kind {
        Inline::Text(t) if end => t.chars().last(),
        Inline::Text(t) => t.chars().next(),
        _ => Some('$'),
    };
    let first = body.iter().find_map(|n| edge(n, false));
    let last = body.iter().rev().find_map(|n| edge(n, true));
    let (Some(first), Some(last)) = (first, last) else {
        return false; // empty
    };
    let boundary = |c: Option<char>, end: bool| match c {
        Some(c) => !c.is_alphanumeric() && c != m,
        None => end,
    };
    !first.is_whitespace()
        && !last.is_whitespace()
        && boundary(before.chars().last(), i == 1)
        && boundary(after.chars().next(), i + 2 == inlines.len())
}

impl Writer {
    /// Run `f` and record the bytes it wrote as the range of `id`.
    fn node(&mut self, id: NodeId, f: impl FnOnce(&mut Self)) {
        let index = self.map.entries.len();
        let start = self.out.len();
        self.map.entries.push((id, start..start));
        f(self);
        self.map.entries[index].1.end = self.out.len();
    }

    /// A line break, indented to the current list level.
    fn nl(&mut self) {
        self.out.push('\n');
        let indent = self.indent.clone();
        self.out.push_str(&indent);
    }

    /// The blocks of a body, a blank line apart; consecutive items (and an
    /// item right after text) are a line apart, so the list stays tight.
    fn blocks(&mut self, blocks: &[BlockNode]) {
        for (i, child) in blocks.iter().enumerate() {
            if i > 0 {
                if !matches!(child.kind, Block::Item { .. }) {
                    self.out.push('\n');
                }
                self.nl();
            }
            self.block(child);
        }
    }

    fn block(&mut self, block: &BlockNode) {
        self.node(block.id, |w| w.block_kind(block));
        // The label, as in Typst: right after what it labels.
        if !block.label.is_empty() && block.kind.labelable() {
            self.out.push_str(" <");
            self.out.push_str(&block.label);
            self.out.push('>');
        }
    }

    fn block_kind(&mut self, block: &BlockNode) {
        let w = self;
        match &block.kind {
            Block::Heading { level, inlines } => {
                for _ in 0..(*level).max(1) {
                    w.out.push('=');
                }
                w.out.push(' ');
                w.text_block(inlines, false);
            }
            // A call being typed (`#align(cen`) shows in a code font in the
            // editor: space or `(` will make it a call.
            Block::Paragraph(inlines) => w.text_block(inlines, true),
            Block::DisplayMath(row) => {
                w.out.push_str("$ ");
                w.row(row);
                w.out.push_str(" $");
            }
            Block::RawTypst(src) => {
                let indent = w.indent.clone();
                w.verbatim(block.id, src, &indent);
            }
            Block::Diagram(src) => {
                if w.style == Style::Edit {
                    // Standalone block documents import CeTZ themselves; the
                    // whole document imports it once at the top.
                    w.out.push_str(CETZ_IMPORT);
                    w.nl();
                }
                w.out.push_str("#align(center, cetz.canvas({");
                w.nl();
                w.out.push_str("  import cetz.draw: *");
                w.nl();
                w.out.push_str("  ");
                let indent = format!("{}  ", w.indent);
                w.verbatim(block.id, src, &indent);
                w.nl();
                w.out.push_str("}))");
            }
            Block::Env {
                name,
                args,
                title,
                body,
            } => {
                // Failed to compile: the call's source on red around its
                // title and body, which stay text.
                if w.style == Style::Edit && w.failed.contains(&block.id) {
                    w.out.push_str(&failed_source(&call_head(name, args)));
                    if !title.is_empty() {
                        w.out.push_str(&failed_source("["));
                        w.text_block(title, false);
                        w.out.push_str(&failed_source("]"));
                    }
                    w.out.push_str(&failed_source("["));
                    w.nl();
                    w.blocks(body);
                    w.nl();
                    w.out.push_str(&failed_source("]"));
                    return;
                }
                w.out.push('#');
                w.out.push_str(name);
                // A figure's caption, the last block of its body, is its
                // `caption:` argument. An empty one shows only in the editor.
                let (body, caption) = match body.split_last() {
                    Some((
                        last @ BlockNode {
                            kind: Block::Caption(c),
                            ..
                        },
                        rest,
                    )) if !(w.style == Style::Readable && is_empty_text(c)) => (rest, Some(last)),
                    Some((
                        BlockNode {
                            kind: Block::Caption(_),
                            ..
                        },
                        rest,
                    )) => (rest, None),
                    _ => (&body[..], None),
                };
                // A figure of a picture: the picture is its first argument,
                // `#figure(image("…"), caption: […])`.
                if let (
                    [
                        picture @ BlockNode {
                            kind: Block::Image { .. },
                            ..
                        },
                    ],
                    "figure",
                ) = (body, name.as_str())
                {
                    w.out.push('(');
                    w.node(picture.id, |w| w.image(&picture.kind));
                    for arg in [&args[..]].into_iter().filter(|a| !a.is_empty()) {
                        w.out.push_str(", ");
                        w.out.push_str(arg);
                    }
                    if let Some(caption) = caption {
                        w.out.push_str(", caption: [");
                        w.block(caption);
                        w.out.push(']');
                    }
                    w.out.push(')');
                    return;
                }
                if !args.is_empty() || caption.is_some() {
                    w.out.push('(');
                    w.out.push_str(args);
                    if let Some(caption) = caption {
                        if !args.is_empty() {
                            w.out.push_str(", ");
                        }
                        w.out.push_str("caption: [");
                        w.block(caption);
                        w.out.push(']');
                    }
                    w.out.push(')');
                }
                // The title is the first content argument, as in ctheorems
                // and theorion: `#lemma[Title][…]`. An empty one (being
                // typed) shows only in the editor.
                let empty = matches!(&title[..], [InlineNode { kind: Inline::Text(s), .. }] if s.is_empty());
                if !title.is_empty() && !(empty && w.style == Style::Readable) {
                    w.out.push('[');
                    w.text_block(title, false);
                    w.out.push(']');
                }
                w.out.push('[');
                w.nl();
                w.blocks(body);
                w.nl();
                w.out.push(']');
            }
            Block::Image { .. } => {
                w.out.push('#');
                w.image(&block.kind);
            }
            // In a figure, written by the figure; elsewhere (not made by the
            // editor) like a paragraph.
            Block::Caption(inlines) => w.text_block(inlines, false),
            // The export names the project's `refs.bib` (written from the
            // block); the editor's renders embed the source.
            Block::Bibliography(source) => match w.style {
                Style::Readable => w
                    .out
                    .push_str(&format!("#bibliography({})", quoted(crate::bib::BIB_FILE))),
                Style::Edit => w.out.push_str(&crate::bib::bibliography_code(source)),
            },
            // The editor renders the block alone, so it sets the metadata
            // itself; the export sets it at the top of the file.
            Block::Title(source) => {
                let fields = TitleFields::parse(source);
                if w.style == Style::Edit {
                    w.out.push_str(&document_metadata(&fields));
                    w.nl();
                }
                w.out.push_str(&title_content(&fields, w.style));
            }
            Block::Item { kind, term, body } => {
                let number = w.number.take().filter(|_| w.style == Style::Edit);
                match (kind, number) {
                    (ListKind::Bullet, _) => w.out.push_str("- "),
                    (ListKind::Numbered, Some(n)) => w.out.push_str(&format!("{n}. ")),
                    (ListKind::Numbered, None) => w.out.push_str("+ "),
                    (ListKind::Term, _) => {
                        w.out.push_str("/ ");
                        w.text_block(term, false);
                        w.out.push_str(": ");
                    }
                }
                // The body's lines are indented past the marker.
                let outer = w.indent.clone();
                w.indent.push_str("  ");
                w.blocks(body);
                w.indent = outer;
            }
        }
    }

    /// Typst source copied as is, each line after the first indented by
    /// `indent`; records where every char went.
    /// A picture's call, `image("images/x.png", width: 80%)`.
    fn image(&mut self, kind: &Block) {
        let Block::Image { path, opts } = kind else {
            return;
        };
        self.out.push_str("image(");
        self.out.push_str(&quoted(path));
        if !opts.trim().is_empty() {
            self.out.push_str(", ");
            self.out.push_str(opts.trim());
        }
        self.out.push(')');
    }

    fn verbatim(&mut self, id: NodeId, src: &str, indent: &str) {
        let mut starts = Vec::with_capacity(src.len() + 1);
        for c in src.chars() {
            starts.push(self.out.len());
            self.out.push(c);
            if c == '\n' {
                self.out.push_str(indent);
            }
        }
        starts.push(self.out.len());
        self.map.texts.push((id, starts));
    }

    /// The inlines of a paragraph or heading. In the editor's renders an
    /// empty one shows a transparent `x` at its only caret position, so it
    /// takes its line and its glyph says where typed text will appear
    /// (after a heading's number, after "Lemma 1." in an environment).
    fn text_block(&mut self, inlines: &[InlineNode], at_line_start: bool) {
        match inlines {
            [
                InlineNode {
                    id,
                    kind: Inline::Text(s),
                },
            ] if s.is_empty() && self.style == Style::Edit => {
                self.node(*id, |w| {
                    w.out.push_str("#text(fill: rgb(0, 0, 0, 0))[");
                    w.map.texts.push((*id, vec![w.out.len()]));
                    w.out.push_str("x]");
                });
            }
            _ => self.inlines(inlines, at_line_start),
        }
    }

    fn inlines(&mut self, inlines: &[InlineNode], at_line_start: bool) {
        for (i, inline) in inlines.iter().enumerate() {
            let line_start = at_line_start && i == 0;
            self.node(inline.id, |w| match &inline.kind {
                Inline::Text(s) => w.text(inline.id, s, line_start, i + 1 == inlines.len()),
                Inline::Raw(s) => w.raw(inline.id, s),
                Inline::Code(s) => {
                    let next = match inlines.get(i + 1).map(|n| &n.kind) {
                        Some(Inline::Text(t)) => t.chars().next(),
                        _ => None,
                    };
                    w.code(inline.id, s, next);
                }
                Inline::Ref(label) => {
                    let next = match inlines.get(i + 1).map(|n| &n.kind) {
                        Some(Inline::Text(t)) => t.as_str(),
                        _ => "",
                    };
                    w.reference(label, next);
                }
                Inline::Math(row) => {
                    w.out.push('$');
                    w.row(row);
                    w.out.push('$');
                }
                Inline::Call { name, args, body } => {
                    let markup = match name.as_str() {
                        "strong" if args.is_empty() => Some('*'),
                        "emph" if args.is_empty() => Some('_'),
                        _ => None,
                    };
                    match markup {
                        Some(m)
                            if w.style == Style::Readable
                                && !w.markup.contains(&m)
                                && markup_ok(inlines, i, m) =>
                        {
                            w.out.push(m);
                            w.markup.push(m);
                            w.inlines(body, false);
                            w.markup.pop();
                            w.out.push(m);
                        }
                        _ if w.style == Style::Edit && w.failed.contains(&inline.id) => {
                            let head = failed_source(&format!("{}[", call_head(name, args)));
                            w.out.push_str(&head);
                            w.text_block(body, false);
                            w.out.push_str(&failed_source("]"));
                        }
                        _ => {
                            w.out.push_str(&call_head(name, args));
                            w.out.push('[');
                            w.text_block(body, false);
                            w.out.push(']');
                            // A `.` or `(` right after would continue the
                            // expression (`#f[…].x`, `#f[…](…)`), and a `;`
                            // would end it and vanish.
                            let next = inlines.get(i + 1).and_then(|n| match &n.kind {
                                Inline::Text(t) => t.chars().next(),
                                _ => None,
                            });
                            if matches!(next, Some('.' | '(' | ';')) {
                                w.out.push(';');
                            }
                        }
                    }
                }
            });
        }
    }

    /// A text inline; `last` if it ends its inline list (a line break at
    /// its end then leaves an empty line, which the editor shows).
    fn text(&mut self, id: NodeId, s: &str, line_start: bool, last: bool) {
        let mut starts = Vec::with_capacity(s.len() + 1);
        let chars: Vec<char> = s.chars().collect();
        // "1." at the start of a line would become a numbered list.
        let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
        let enum_dot = line_start && digits > 0 && chars.get(digits) == Some(&'.');
        // An empty line after a line break shows a transparent `x` in the
        // editor, at the position after the break, so the caret has a place.
        let mut placeholder = false;
        // Calls being typed show in the code font, in the editor only.
        let code = match self.style {
            Style::Edit => calls_being_typed(&chars),
            _ => vec![],
        };
        for (i, &c) in chars.iter().enumerate() {
            starts.push(self.out.len());
            if std::mem::take(&mut placeholder) {
                self.out.push_str("x]");
            }
            if code.iter().any(|&(start, _)| start == i) {
                self.out.push_str(CODE_TEXT);
            }
            let next = chars.get(i + 1).copied();
            let escape = match c {
                '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '@' | '[' | ']' | '~' => true,
                // Markup that only triggers at the start of a line.
                '=' | '+' | '-' | '/' if line_start && i == 0 => true,
                '.' => enum_dot && i == digits,
                // `--`/`---` dashes, `-?` soft hyphen, `//` and `/*` comments.
                '-' => matches!(next, Some('-' | '?')),
                '/' => matches!(next, Some('/' | '*')),
                _ => false,
            };
            match c {
                // A line break; written with a space so it also works in a
                // heading, where a newline would end it.
                '\n' => {
                    self.out.push_str("\\ ");
                    let empty_line = next == Some('\n') || next.is_none() && last;
                    if empty_line && self.style == Style::Edit {
                        self.out.push_str("#text(fill: rgb(0, 0, 0, 0))[");
                        placeholder = true;
                    }
                }
                '\r' => self.out.push(' '),
                '\u{a0}' => self.out.push('~'),
                _ if escape => {
                    self.out.push('\\');
                    self.out.push(c);
                }
                _ => self.out.push(c),
            }
            if code.iter().any(|&(_, end)| end == i + 1) {
                self.out.push(']');
            }
        }
        starts.push(self.out.len());
        if placeholder {
            self.out.push_str("x]");
        }
        self.map.texts.push((id, starts));
    }

    /// A reference, `@label`, followed by the text `next`. In the editor it
    /// is wrapped in `sem-code`, whose box carries its span (the glyphs come
    /// from the target's supplement and numbering); one with no label yet
    /// shows a grey `@`. Exported as `@label`, or `#ref(<label>)` when the
    /// text after it would continue the label.
    fn reference(&mut self, label: &str, next: &str) {
        let mut chars = next.chars();
        let continues =
            |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || "-_.:".contains(c));
        let first = chars.next();
        let joins = match first {
            Some('.' | ':') => continues(chars.next()),
            c => continues(c),
        };
        match (self.style, label.is_empty()) {
            (Style::Edit, true) => self.out.push_str("#sem-code[#text(fill: luma(140))[\\@]]"),
            (Style::Edit, false) => {
                self.out.push_str("#sem-code[@");
                self.out.push_str(label);
                self.out.push(']');
            }
            (Style::Readable, true) => self.out.push_str("\\@"),
            (Style::Readable, false) if joins => {
                self.out.push_str("#ref(<");
                self.out.push_str(label);
                self.out.push_str(">)");
            }
            (Style::Readable, false) => {
                self.out.push('@');
                self.out.push_str(label);
            }
        }
    }

    /// Inline code, `` `…` ``, with every char mapped. Empty code shows a
    /// transparent `x` in the editor; code holding a backtick is written as
    /// a `raw` call.
    /// A code piece in text, `#source` (`s` without its `#`), followed by
    /// `next`. Exported as is, with a `;` before a char that would continue
    /// the expression (it ends it and vanishes). In the editor, when open
    /// its source shows in the code font, each char in place; else what it
    /// draws, between empty `sem-code` boxes that carry its span.
    fn code(&mut self, id: NodeId, s: &str, next: Option<char>) {
        let end = |w: &mut Self| {
            if next.is_some_and(|c| !c.is_whitespace()) {
                w.out.push(';');
            }
        };
        let failed = self.failed.contains(&id);
        match self.style {
            Style::Edit if self.open == Some(id) || failed => {
                if failed {
                    self.out.push_str(FAILED);
                }
                self.out.push_str(CODE_TEXT);
                self.out.push_str("\\#");
                self.text(id, s, false, false);
                self.out.push(']');
                if failed {
                    self.out.push(']');
                }
            }
            Style::Edit => {
                self.out.push_str("#sem-code()#");
                self.out.push_str(if s.is_empty() { "none" } else { s });
                self.out.push_str("#sem-code()");
                end(self);
            }
            Style::Readable if s.is_empty() => {}
            Style::Readable => {
                self.out.push('#');
                self.out.push_str(s);
                end(self);
            }
        }
    }

    fn raw(&mut self, id: NodeId, s: &str) {
        let mut starts = Vec::with_capacity(s.len() + 1);
        if s.is_empty() && self.style == Style::Edit {
            self.out.push_str("#text(fill: rgb(0, 0, 0, 0))[");
            starts.push(self.out.len());
            self.out.push_str("x]");
            self.map.texts.push((id, starts));
            return;
        }
        let call = s.is_empty() || s.contains('`');
        self.out.push_str(if call { "#raw(\"" } else { "`" });
        for c in s.chars() {
            starts.push(self.out.len());
            match c {
                '"' | '\\' if call => {
                    self.out.push('\\');
                    self.out.push(c);
                }
                '\n' if call => self.out.push_str("\\n"),
                _ => self.out.push(c),
            }
        }
        starts.push(self.out.len());
        self.out.push_str(if call { "\")" } else { "`" });
        self.map.texts.push((id, starts));
    }

    fn row(&mut self, row: &MathRow) {
        self.node(row.id, |w| {
            if row.nodes.is_empty() {
                w.out.push_str(SLOT);
            }
            let paired = paired_delimiters(&row.nodes);
            let n = &row.nodes;
            let spaced: Vec<bool> = (0..n.len())
                .map(|i| i > 0 && w.space_between(&n[i - 1], &n[i], paired[i - 1], paired[i]))
                .collect();
            for (i, node) in row.nodes.iter().enumerate() {
                if spaced[i] {
                    w.out.push(' ');
                }
                match (&node.kind, paired[i]) {
                    // A line left empty after a break holds a placeholder,
                    // so it shows and the caret has a place on it.
                    (MathKind::Linebreak, _)
                        if w.style == Style::Edit
                            && row
                                .nodes
                                .get(i + 1)
                                .is_none_or(|n| n.kind == MathKind::Linebreak) =>
                    {
                        w.node(node.id, |w| {
                            w.out.push_str("\\ ");
                            w.out.push_str(SLOT);
                        });
                    }
                    // Literal, so Typst scales the pair to its content.
                    (MathKind::Sym(d), true) => w.node(node.id, |w| w.out.push_str(d)),
                    // A bar is written `|`, escaped where it would form a
                    // shorthand with what is written tight around it
                    // (`||`, `[|`, `|]`, `|->`, `|=>`).
                    (MathKind::Sym(b), _) if b == "|" => {
                        let next = n
                            .get(i + 1)
                            .filter(|_| !spaced.get(i + 1).copied().unwrap_or(true));
                        let clash = w.out.ends_with(['[', '|'])
                            || next.is_some_and(|m| match &m.kind {
                                MathKind::Sym(s) => s.starts_with(['|', ']', '-', '=']),
                                _ => false,
                            });
                        w.node(node.id, |w| w.out.push_str(if clash { "\\|" } else { "|" }));
                    }
                    _ => w.math(node),
                }
            }
        });
    }

    /// Whether to put a space between two neighbouring nodes. Spaces between
    /// atoms do not change Typst's output; the readable style drops them
    /// only next to brackets and punctuation, where no identifier can form.
    fn space_between(&self, a: &MathNode, b: &MathNode, a_paired: bool, b_paired: bool) -> bool {
        // Primes attach only right after their base; after a fraction they
        // would attach to the denominator, after code be read as part of it.
        if matches!(b.kind, MathKind::Primes(_)) {
            return matches!(a.kind, MathKind::Frac { .. }) || a.source().is_some();
        }
        // Next to a bar or quoted text a space shows (measured): written
        // as the tree keeps it. (An empty text is written as code in the
        // editor, which a bracket would continue.)
        if spaced_gap(a, b)
            && !(self.style == Style::Edit && a.kind == MathKind::Text(String::new()))
        {
            return b.space;
        }
        // In the editor code is a box, and a space next to a box shows
        // (measured: `#sym.brace.l h` got a gap). Its `;` ends it, so
        // nothing after can continue it; before it only `\` would clash.
        let code = |n: &MathNode| matches!(n.kind, MathKind::Code(_));
        if self.style == Style::Edit && (code(a) || code(b)) {
            return a.kind == MathKind::Linebreak;
        }
        // After code, a `;` or `,` would be read as part of it; after `\`
        // they would be escaped.
        if self.style == Style::Edit || a.source().is_some() || a.kind == MathKind::Linebreak {
            return true;
        }
        let sym = |n: &MathNode, set: &[&str]| matches!(&n.kind, MathKind::Sym(s) if set.contains(&s.as_str()));
        let opens = a_paired && sym(a, &["(", "[", "{"]);
        let closes = b_paired && sym(b, &[")", "]", "}"]);
        let punct = self.args == 0 && sym(b, &[",", ";"]);
        // `f(x)`: a single letter is never a function call in Typst math.
        let applied = b_paired
            && sym(b, &["("])
            && matches!(&a.kind, MathKind::Ident(s) if s.chars().count() == 1);
        !(opens || closes || punct || applied)
    }

    /// An inline diagram's code: its drawing in a canvas sized like the
    /// text, in a box raised to sit on the line.
    fn inline_diagram(&mut self, src: &str) {
        self.out
            .push_str("box(baseline: 30%, cetz.canvas(length: 1em, {");
        self.nl();
        self.out.push_str("  import cetz.draw: *");
        for line in src.lines() {
            self.nl();
            self.out.push_str("  ");
            self.out.push_str(line);
        }
        self.nl();
        self.out.push_str("}))");
    }

    /// A row as a fraction part or script: bare if it is one atom (readable
    /// style), else in parentheses, which Typst removes.
    fn group(&mut self, row: &MathRow) {
        if self.style == Style::Readable && is_atom(row) {
            self.row(row);
        } else {
            self.out.push('(');
            self.row(row);
            self.out.push(')');
        }
    }

    fn math(&mut self, node: &MathNode) {
        self.node(node.id, |w| match &node.kind {
            MathKind::Sym(s) => {
                let literal =
                    w.style == Style::Readable && w.args == 0 && matches!(s.as_str(), "," | ";");
                w.out
                    .push_str(&if literal { s.clone() } else { sym_source(s) });
            }
            MathKind::Ident(s) => w.out.push_str(&ident_source(s, w.style)),
            MathKind::Num(s) => w.out.push_str(&num_source(s)),
            MathKind::Slot => w.out.push_str(SLOT),
            // Open (the caret in it) or failed to compile: its source in a
            // box, each char in place (on red if failed).
            MathKind::Code(src)
                if w.style == Style::Edit
                    && (w.open == Some(node.id) || w.failed.contains(&node.id)) =>
            {
                let failed = w.failed.contains(&node.id);
                w.out.push_str("#sem-code[");
                if failed {
                    w.out.push_str(FAILED);
                }
                w.out.push_str(CODE_TEXT);
                w.out.push_str("\\#");
                w.text(node.id, src, false, false);
                w.out.push(']');
                if failed {
                    w.out.push(']');
                }
                w.out.push_str("];");
            }
            MathKind::Code(src) if w.style == Style::Edit => {
                w.out.push_str("#sem-code($#");
                w.out.push_str(&math_code(src));
                w.out.push_str("$);");
            }
            MathKind::Code(src) => {
                w.out.push('#');
                w.out.push_str(&math_code(src));
            }
            MathKind::Primes(n) => w.out.push_str(&"'".repeat(*n)),
            // An empty text (being typed) holds its place in the editor.
            MathKind::Text(s) if s.is_empty() && w.style == Style::Edit => {
                w.out.push_str("#text(fill: rgb(0, 0, 0, 0), \"x\")");
            }
            MathKind::Text(s) => w.out.push_str(&quoted(s)),
            MathKind::Align => w.out.push('&'),
            MathKind::Linebreak => w.out.push('\\'),
            MathKind::Call { name, opts, args } => {
                // Inside the arguments a comma is written `comma`, so it
                // cannot split them.
                // A function of the document's own draws with the spans of
                // its definition: in the editor, empty `sem-code` boxes on
                // both sides carry the call's span, so its box covers all of
                // it. `class` keeps it one piece (for scripts) in the
                // formula's style (a box around it would not). No spaces
                // next to the boxes: after code they are spaces in the
                // formula.
                let wrap = w.style == Style::Edit && !crate::calls::in_math_library(name);
                if wrap {
                    w.out.push_str("class(\"normal\", #sem-code()");
                }
                w.args += 1;
                w.out.push_str(name);
                w.out.push('(');
                w.out.push_str(&math_opts(opts));
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 || !opts.is_empty() {
                        w.out.push_str(", ");
                    }
                    w.row(arg);
                }
                w.out.push(')');
                w.args -= 1;
                if wrap {
                    w.out.push_str("#sem-code())");
                }
            }
            MathKind::Diagram(src) if w.style == Style::Edit => {
                w.out.push_str("#sem-code($#");
                w.inline_diagram(src);
                w.out.push_str("$)");
            }
            MathKind::Diagram(src) => {
                w.out.push('#');
                w.inline_diagram(src);
            }
            MathKind::Frac { num, den } => {
                w.group(num);
                w.out.push('/');
                w.group(den);
            }
            MathKind::Scripts { base, sup, sub } => {
                if is_simple_base(base) || w.style == Style::Readable && is_bracket_group(base) {
                    // A bracketed group takes scripts as a whole and keeps
                    // its brackets, as `attach` does.
                    w.row(base);
                    if let Some(sup) = sup {
                        w.out.push('^');
                        w.group(sup);
                    }
                    if let Some(sub) = sub {
                        w.out.push('_');
                        w.group(sub);
                    }
                } else {
                    // `(a b)^2` would show the parentheses; attach() does not.
                    w.args += 1;
                    w.out.push_str("attach(");
                    w.row(base);
                    if let Some(sup) = sup {
                        w.out.push_str(", t: ");
                        w.row(sup);
                    }
                    if let Some(sub) = sub {
                        w.out.push_str(", b: ");
                        w.row(sub);
                    }
                    w.out.push(')');
                    w.args -= 1;
                }
            }
        });
    }
}

/// Which nodes of a row are brackets that pair up. Typst pairs any literal
/// opening `( [ {` with the next unpaired closing `) ] }` (so `[a, b)` works)
/// and scales the pair. Paired brackets are emitted literally; unpaired ones
/// as names (`paren.l`), which cannot break the grouping around them.
fn paired_delimiters(nodes: &[MathNode]) -> Vec<bool> {
    let mut paired = vec![false; nodes.len()];
    let mut open = vec![];
    for (i, node) in nodes.iter().enumerate() {
        let MathKind::Sym(s) = &node.kind else {
            continue;
        };
        match s.as_str() {
            "(" | "[" | "{" => open.push(i),
            ")" | "]" | "}" => {
                if let Some(j) = open.pop() {
                    paired[i] = true;
                    paired[j] = true;
                }
            }
            _ => {}
        }
    }
    paired
}

/// A row that can stand without parentheses as a fraction part or script:
/// one number, single-letter or quoted identifier, letter-like symbol, or
/// slot.
fn is_atom(row: &MathRow) -> bool {
    match row.nodes.as_slice() {
        [] => true, // an empty slot
        [node] => match &node.kind {
            MathKind::Num(_) | MathKind::Slot => true,
            MathKind::Ident(s) => !OPERATORS.iter().any(|(name, ..)| name == s),
            MathKind::Sym(s) => s.chars().count() == 1 && s.chars().all(char::is_alphanumeric),
            _ => false,
        },
        _ => false,
    }
}

/// A row that is one bracket pair around the rest: `(a b)`, `[0, 1)`.
fn is_bracket_group(row: &MathRow) -> bool {
    let n = row.nodes.len();
    if n < 2 {
        return false;
    }
    // The first bracket must pair with the last one, not close earlier.
    let mut depth = 0;
    for (i, node) in row.nodes.iter().enumerate() {
        match &node.kind {
            MathKind::Sym(s) if matches!(s.as_str(), "(" | "[" | "{") => depth += 1,
            MathKind::Sym(s) if matches!(s.as_str(), ")" | "]" | "}") => {
                depth -= 1;
                if depth == 0 && i + 1 < n {
                    return false;
                }
            }
            _ if i == 0 => return false,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

/// A base that can take `^`/`_` directly: exactly one atom.
fn is_simple_base(base: &MathRow) -> bool {
    matches!(
        base.nodes.as_slice(),
        [MathNode {
            kind: MathKind::Sym(_) | MathKind::Ident(_) | MathKind::Num(_),
            ..
        }]
    )
}

/// The document's metadata from a title block, `#set document(title: […],
/// author: (…), date: …)`: the PDF's title, authors and date.
pub fn document_metadata(fields: &TitleFields) -> String {
    let mut args = vec![];
    if !fields.title.trim().is_empty() {
        args.push(format!("title: [{}]", fields.title));
    }
    if !fields.authors.is_empty() {
        let names: Vec<String> = fields.authors.iter().map(|a| quoted(a)).collect();
        // A one-element array needs its trailing comma.
        let comma = if names.len() == 1 { "," } else { "" };
        args.push(format!("author: ({}{comma})", names.join(", ")));
    }
    if let Some((year, month, day)) = fields.ymd() {
        args.push(format!(
            "date: datetime(year: {year}, month: {month}, day: {day})"
        ));
    }
    format!("#set document({})", args.join(", "))
}

/// The title block's author line: the document's authors, from its
/// metadata.
pub(crate) const TITLE_AUTHORS: &str = "#context document.author.join(\", \")";

/// What a title block shows: the title (Typst's `title` element, styled by
/// the header's `show title` rules) and a line with the authors. In the
/// editor, an empty one shows a grey "Title".
fn title_content(fields: &TitleFields, style: Style) -> String {
    let mut lines = vec![];
    if !fields.title.trim().is_empty() {
        lines.push("#title()".to_string());
    }
    if !fields.authors.is_empty() {
        lines.push(TITLE_AUTHORS.to_string());
    }
    if lines.is_empty() && style == Style::Edit {
        lines.push("#text(fill: luma(140))[Title]".to_string());
    }
    lines.join("\n\n")
}

/// The options of a math call (code, `size: 55%`) as written in math:
/// each value after a `#` (`size: #55%`), in parentheses when it is not one
/// expression a `#` can take (`#(80% + 2pt)`); strings stay as they are.
/// Options that are not all named arguments are written as typed (Typst
/// then reports the error).
fn math_opts(opts: &str) -> String {
    let call = typst::syntax::parse_code(&format!("f({opts})"));
    let args = call
        .children()
        .find_map(|c| c.cast::<ast::FuncCall>())
        .filter(|_| call.errors_and_warnings().0.is_empty())
        .map(|c| c.args().items().collect::<Vec<_>>());
    let Some(args) = args else {
        return opts.to_string();
    };
    let mut out = vec![];
    for arg in args {
        let ast::Arg::Named(named) = arg else {
            return opts.to_string();
        };
        let expr = named.expr();
        let value = expr.to_untyped().full_text();
        let value = match expr {
            ast::Expr::Str(_) => value.to_string(),
            e if e.hash() => format!("#{value}"),
            _ => format!("#({value})"),
        };
        out.push(format!("{}: {value}", named.name().as_str()));
    }
    out.join(", ")
}

/// `s` as a Typst string literal.
pub fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn sym_source(s: &str) -> String {
    let mut chars = s.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        // Several chars: raw only if they cannot form an identifier.
        let raw = !s.is_empty() && s.chars().all(|c| !c.is_ascii() && !c.is_alphanumeric());
        return if raw { s.into() } else { quoted(s) };
    };
    let name = match c {
        // Would break grouping or argument lists if emitted raw.
        ',' => "comma",
        ';' => "semi",
        '(' => "paren.l",
        ')' => "paren.r",
        '[' => "bracket.l",
        ']' => "bracket.r",
        '{' => "brace.l",
        '}' => "brace.r",
        '|' => "\\|",
        '/' => "slash",
        '&' => "amp",
        '"' => "quote.double",
        '~' => "tilde.op",
        _ => "",
    };
    if !name.is_empty() {
        name.into()
    } else if c.is_alphanumeric()
        || matches!(
            c,
            '+' | '-' | '=' | '<' | '>' | '!' | '\'' | '.' | ':' | '?' | '*'
        )
    {
        c.into()
    } else if c.is_ascii() {
        format!("\\{c}")
    } else {
        c.into()
    }
}

/// The code of a code atom in a formula: a name only Typst's math module
/// defines (`dif`, typed as a word) is `math.dif`, since code after `#`
/// does not see the math module.
fn math_code(src: &str) -> String {
    let name = src
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    if name && crate::symbols::math_only(src) {
        format!("math.{src}")
    } else {
        src.to_string()
    }
}

/// An identifier: a letter as is; a predefined operator ([`OPERATORS`])
/// as `op(#[log])` in the editor, since the predefined ones take their text
/// from the library, so their glyphs would carry library spans and could
/// not be mapped back to the node (`op("log")` loses the span too, a string
/// argument being a value; a markup block keeps it; same rendering); other
/// words quoted (older documents; typed words become code now).
fn ident_source(s: &str, style: Style) -> String {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_alphabetic() => s.into(),
        _ if style == Style::Readable && OPERATORS.iter().any(|(name, ..)| *name == s) => s.into(),
        _ => match OPERATORS.iter().find(|(name, ..)| *name == s) {
            // `liminf`'s thin space is a literal U+2009 in the text, as in
            // Typst's own definition, so the text stays one run.
            Some((_, text, limits)) => {
                let limits = if *limits { ", limits: #true" } else { "" };
                format!("op(#[{text}]{limits})")
            }
            None => quoted(s),
        },
    }
}

fn num_source(s: &str) -> String {
    let valid = s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
        && !s.ends_with('.');
    if valid { s.into() } else { quoted(s) }
}

/// Serialize one block into a standalone snippet and its span map.
pub fn serialize_block(block: &BlockNode) -> (String, SpanMap) {
    serialize_block_styled(block, Style::Edit)
}

/// Serialize one block in the given style.
pub fn serialize_block_styled(block: &BlockNode, style: Style) -> (String, SpanMap) {
    serialize_with(block, style, None, None, &[])
}

/// Serialize one block for the editor; a numbered list item shows `number`
/// (its place in the list, as the block is rendered on its own).
pub fn serialize_block_numbered(block: &BlockNode, number: Option<usize>) -> (String, SpanMap) {
    serialize_with(block, Style::Edit, number, None, &[])
}

/// [`serialize_block_numbered`], with the source of code piece `open` (the
/// caret is in it) showing instead of what it draws, and the source of the
/// code pieces and calls that `failed` to compile showing on red.
pub fn serialize_block_open(
    block: &BlockNode,
    number: Option<usize>,
    open: Option<NodeId>,
    failed: &[NodeId],
) -> (String, SpanMap) {
    serialize_with(block, Style::Edit, number, open, failed)
}

/// A standalone snippet showing only math node `node` (an inline diagram
/// or code atom being edited), for the source dialog's preview.
pub fn serialize_math_node(node: &MathNode) -> String {
    let mut w = Writer {
        out: String::new(),
        map: SpanMap::default(),
        style: Style::Readable,
        args: 0,
        indent: String::new(),
        number: None,
        markup: vec![],
        open: None,
        failed: vec![],
    };
    if matches!(node.kind, MathKind::Diagram(_))
        || matches!(&node.kind, MathKind::Code(s) if s.contains("cetz"))
    {
        w.out.push_str(CETZ_IMPORT);
        w.out.push('\n');
    }
    w.out.push('$');
    w.math(node);
    w.out.push('$');
    w.out
}

/// Whether a block draws with CeTZ: a diagram (unless `diagrams` is
/// false), or code in a formula that mentions it (an inline diagram), here
/// or in a container's title or body.
pub(crate) fn uses_cetz(block: &BlockNode, diagrams: bool) -> bool {
    fn in_row(row: &MathRow) -> bool {
        row.nodes.iter().any(|n| match &n.kind {
            MathKind::Code(src) => src.contains("cetz"),
            MathKind::Diagram(_) => true,
            _ => n.children().into_iter().any(|(_, r)| in_row(r)),
        })
    }
    diagrams && matches!(block.kind, Block::Diagram(_))
        || block.rows().into_iter().any(in_row)
        || block
            .kind
            .body()
            .is_some_and(|body| body.iter().any(|b| uses_cetz(b, diagrams)))
}

fn serialize_with(
    block: &BlockNode,
    style: Style,
    number: Option<usize>,
    open: Option<NodeId>,
    failed: &[NodeId],
) -> (String, SpanMap) {
    let mut out = String::new();
    // Standalone block documents import CeTZ themselves; a diagram block
    // does so in its own code.
    if style == Style::Edit && uses_cetz(block, false) {
        out.push_str(CETZ_IMPORT);
        out.push('\n');
    }
    let mut w = Writer {
        out,
        map: SpanMap::default(),
        style,
        args: 0,
        indent: String::new(),
        number,
        markup: vec![],
        open,
        failed: failed.to_vec(),
    };
    w.block(block);
    (w.out, w.map)
}

/// The whole document as one readable Typst file: preamble, header, blocks.
/// A copied piece as Typst, written as the export writes it: blocks joined
/// as in a document (no preamble or header), inlines as a paragraph's text,
/// math nodes as a formula's content (without `$`).
pub fn fragment_source(fragment: &Fragment) -> String {
    let mut d = Document::empty();
    let paragraph = |p: BlockNode| serialize_block_styled(&p, Style::Readable).0;
    match fragment {
        Fragment::Plain(s) => s.clone(),
        Fragment::Inlines(inlines) => paragraph(d.paragraph(inlines.clone())),
        Fragment::Math(nodes) => {
            let m = d.math(nodes.clone());
            let s = paragraph(d.paragraph(vec![m]));
            let s = s.trim();
            let s = s.strip_prefix('$').unwrap_or(s);
            s.strip_suffix('$').unwrap_or(s).trim().to_string()
        }
        Fragment::Blocks(blocks) => {
            let item = |b: &BlockNode| matches!(b.kind, Block::Item { .. });
            let mut out = String::new();
            for (i, block) in blocks.iter().enumerate() {
                if i > 0 && !item(block) {
                    out.push('\n');
                }
                out.push_str(&serialize_block_styled(block, Style::Readable).0);
                out.push('\n');
            }
            out.trim_end().to_string()
        }
    }
}

pub fn serialize_document(doc: &Document) -> String {
    serialize_document_mapped(doc).0
}

/// The whole document as one readable Typst file, with a span map over all
/// blocks.
pub fn serialize_document_mapped(doc: &Document) -> (String, SpanMap) {
    let mut out = String::from(PREAMBLE);
    // The metadata first, so the header's rules (a template) can read it.
    if let Some(fields) = doc.title() {
        out.push_str(&document_metadata(&fields));
        out.push('\n');
    }
    out.push_str(&doc.header);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if doc.blocks.iter().any(|b| uses_cetz(b, true)) {
        out.push_str(CETZ_IMPORT);
        out.push('\n');
    }
    let mut map = SpanMap::default();
    let item = |b: &BlockNode| matches!(b.kind, Block::Item { .. });
    for (i, block) in doc.blocks.iter().enumerate() {
        // Consecutive items stay one tight list, and a list starts on the
        // line after the block before it: a blank line there adds space
        // (measured), as in bodies (`Writer::blocks`).
        if i == 0 || !item(block) {
            out.push('\n');
        }

        let (snippet, mut block_map) = serialize_block_styled(block, Style::Readable);
        block_map.shift(out.len());
        map.entries.append(&mut block_map.entries);
        map.texts.append(&mut block_map.texts);
        out.push_str(&snippet);
        out.push('\n');
    }
    (out, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Around, BridgeWorld, block_document, compile};

    fn ser(block: &BlockNode) -> String {
        serialize_block(block).0
    }

    /// Copied pieces as Typst: inlines as text, math nodes without `$`,
    /// blocks as in the export.
    #[test]
    fn fragments_as_typst() {
        let mut d = Document::empty();
        let (a, x, b) = (d.text("a *"), d.ident("x"), d.text(" b"));
        let m = d.math(vec![x]);
        assert_eq!(
            fragment_source(&Fragment::Inlines(vec![a, m, b])),
            "a \\*$x$ b"
        );
        let (x, plus, y) = (d.ident("x"), d.sym("+"), d.ident("y"));
        assert_eq!(fragment_source(&Fragment::Math(vec![x, plus, y])), "x + y");
        let t = d.text("one");
        let p1 = d.paragraph(vec![t]);
        let t = d.text("two");
        let p2 = d.paragraph(vec![t]);
        assert_eq!(
            fragment_source(&Fragment::Blocks(vec![p1, p2])),
            "one\n\ntwo"
        );
    }

    /// Golden cases: (description, block, expected Typst).
    fn golden() -> Vec<(&'static str, BlockNode, &'static str)> {
        let mut d = Document::empty();
        let mut cases = vec![];

        let (s, le, log, chi, two, c) = (
            d.ident("S"),
            d.sym("≤"),
            d.ident("log"),
            d.ident("chi"),
            d.num("2"),
            d.ident("c"),
        );
        let plus = d.sym("+");
        let f = d.frac(vec![log, chi], vec![two]);
        cases.push((
            "entropy",
            d.display(vec![s, le, f, plus, c]),
            "$ S ≤ (op(#[log]) \"chi\")/(2) + c $",
        ));

        let x = d.ident("x");
        let two = d.num("2");
        let i = d.ident("i");
        let sc = d.scripts(vec![x], Some(vec![two]), Some(vec![i]));
        let t1 = d.text("where ");
        let m = d.math(vec![sc]);
        let t2 = d.text(" holds.");
        cases.push((
            "scripts inline",
            d.paragraph(vec![t1, m, t2]),
            "where $x^(2)_(i)$ holds.",
        ));

        let a = d.ident("a");
        let b = d.ident("b");
        let n = d.ident("n");
        let sc = d.scripts(vec![a, b], Some(vec![n]), None);
        cases.push((
            "compound base",
            d.display(vec![sc]),
            "$ attach(a b, t: n) $",
        ));

        let empty_frac = d.frac(vec![], vec![]);
        cases.push((
            "slots",
            d.display(vec![empty_frac]),
            "$ (#sem-slot())/(#sem-slot()) $",
        ));
        let m = d.math(vec![]);
        cases.push(("empty inline math", d.paragraph(vec![m]), "$#sem-slot()$"));

        let syms: Vec<_> = [",", ";", "(", ")", "|", "/", "&", "#", "$", "→", "@"]
            .into_iter()
            .map(|s| d.sym(s))
            .collect();
        cases.push((
            "special symbols",
            d.display(syms),
            "$ comma semi ( )|slash amp \\# \\$ → \\@ $",
        ));

        let t = d.text("1. Use *bold* and _x_ #code $5 @ref <l> [a] a~b // c -- d -? e/f - g");
        cases.push((
            "text escapes",
            d.paragraph(vec![t]),
            // `#code` in the code font of calls being typed, in the editor.
            "1\\. Use \\*bold\\* and \\_x\\_ #text(font: \"DejaVu Sans Mono\", size: 0.9em, fill: rgb(\"#6b6390\"))[\\#code] \\$5 \\@ref \\<l> \\[a\\] a\\~b \\// c \\-- d \\-? e/f - g",
        ));
        let t = d.text("- not a list");
        cases.push(("leading dash", d.paragraph(vec![t]), "\\- not a list"));

        let t = d.text("Intro ");
        let e = d.ident("e");
        let m = d.math(vec![e]);
        cases.push(("heading", d.heading(2, vec![t, m]), "== Intro $e$"));

        let n1 = d.num("3.14");
        let n2 = d.num("1e5");
        let q = d.ident("q\"x");
        let alpha = d.ident("α");
        let leq = d.sym("≤≥");
        cases.push((
            "numbers and quoting",
            d.display(vec![n1, n2, q, alpha, leq]),
            "$ 3.14 \"1e5\" \"q\\\"x\" α ≤≥ $",
        ));

        cases.push(("raw", d.raw("#lorem(5)"), "#lorem(5)"));
        cases
    }

    #[test]
    fn golden_snapshots() {
        for (name, block, want) in golden() {
            assert_eq!(ser(&block), want, "{name}");
        }
    }

    #[test]
    fn every_snapshot_compiles() {
        let mut world = BridgeWorld::new();
        for (name, block, _) in golden() {
            let (src, _) = block_document(&Around::default(), &ser(&block), 300.0);
            if let Err(e) = compile(&mut world, &src) {
                panic!("{name}: {e:?}\n{src}");
            }
        }
    }

    /// Every state while typing formulas key by key compiles, so editing
    /// never shows an error in between.
    #[test]
    fn every_typing_state_compiles() {
        let mut world = BridgeWorld::new();
        let sessions = [
            "S <= (log chi)/(2) + c",
            "\\alpha^2_i/x + \\x ' , ; | [ ] { } & ~~ := \\",
            "(a+b)^2 / ( ) _ ^ 3.14.15 é ∑ #$",
            "e^(-x^2/2) ! != -> <-> 1/2/3",
            "a times.o b arrow.r.long c eq.not x. times.q y. lt.eq.slant",
            "sqrt x+root(3,y)+cal(A),b+cases(a,b,,c)+abs(x,y)+hat(u)+floor(v)/w^2",
            "overline(underbrace(x, n))_k + binom(n,k,) + op(a, b)",
            "f''(x) + a/b' + \"if \\ \"q\" x\"\" &= y \\ &= z \\\\ w'_2",
        ];
        let mut d = Document::empty();
        let p = d.paragraph(vec![]);
        d.blocks = vec![p];
        for keys in sessions {
            let mut e = model::Editor::new(d.clone());
            e.set_symbol_resolver(crate::resolve_symbol);
            e.set_math_call_resolver(crate::math_call_shape);
            e.insert_text("text $");
            for c in keys.chars() {
                e.insert_text(&c.to_string());
                let block = &e.doc().blocks[0];
                let (src, _) = block_document(&Around::default(), &ser(block), 300.0);
                if let Err(err) = compile(&mut world, &src) {
                    panic!("after {c:?} in {keys:?}: {err:?}\n{src}");
                }
            }
        }
    }

    #[test]
    fn paired_brackets_are_literal() {
        let mut d = Document::empty();
        let nodes = ["(", "[", "a", ")", "]", "{", "(", "b", ")"]
            .map(|s| {
                if s.len() == 1 && s.chars().all(char::is_alphabetic) {
                    d.ident(s)
                } else {
                    d.sym(s)
                }
            })
            .to_vec();
        let block = d.display(nodes);
        // `(` pairs with `]`? No: `[` pairs with `)`, `(` with `]`; `{` stays alone.
        assert_eq!(ser(&block), "$ ( [ a ) ] brace.l ( b ) $");
    }

    #[test]
    fn document_map_points_into_the_whole_source() {
        let mut d = Document::empty();
        let t = d.text("Hi ");
        let x = d.ident("x");
        let xid = x.id;
        let m = d.math(vec![x]);
        let p = d.paragraph(vec![t, m]);
        let y = d.ident("y");
        let yid = y.id;
        let disp = d.display(vec![y]);
        d.blocks = vec![p, disp];
        let (src, map) = serialize_document_mapped(&d);
        assert_eq!(src, serialize_document(&d));
        assert_eq!(&src[map.range(xid).unwrap()], "x");
        assert_eq!(&src[map.range(yid).unwrap()], "y");
        assert!(src.ends_with("$ y $\n"));
    }

    /// A longer word that is no symbol, operator or call is code, as in
    /// Typst: Typst's math names (`dif`) and the header's (`bigbx`) work, a
    /// typo fails (and is shown failing); operators stay what they were.
    #[test]
    fn typed_words_are_code() {
        let header = "#let bigbx = math.op(sym.times.square, limits: true)\n";
        let compiles = |keys: &str| {
            let block = typed(keys);
            let around = Around {
                header: header.into(),
                ..Around::default()
            };
            let (src, _) = block_document(&around, &ser(&block), 300.0);
            (
                compile(&mut BridgeWorld::new(), &src).is_ok(),
                serialize_block_styled(&block, Style::Readable).0,
            )
        };
        assert_eq!(compiles("dif x"), (true, "$ #math.dif x $".into()));
        assert_eq!(
            compiles("bigbx_i"),
            (true, "$ attach(#bigbx, b: i) $".into())
        );
        assert_eq!(compiles("log x"), (true, "$ log x $".into()));
        assert_eq!(compiles("dagge"), (false, "$ #dagge $".into()));
    }

    /// Typed formulas, in the editor's model.
    fn typed(keys: &str) -> BlockNode {
        let mut d = Document::empty();
        let b = d.display(vec![]);
        d.blocks = vec![b];
        let mut e = model::Editor::new(d);
        e.set_symbol_resolver(crate::resolve_symbol);
        e.insert_text(keys);
        e.insert_text(" ");
        e.doc().blocks[0].clone()
    }

    fn readable(keys: &str) -> String {
        serialize_block_styled(&typed(keys), Style::Readable).0
    }

    #[test]
    fn readable_style() {
        assert_eq!(readable("1/2"), "$ 1/2 $");
        assert_eq!(readable("(a+b)/2"), "$ (a + b)/2 $");
        assert_eq!(readable("S <= (log chi)/(2) + c"), "$ S ≤ (log χ)/2 + c $");
        assert_eq!(readable("x^2"), "$ x^2 $");
        assert_eq!(readable("x^(n+1)_i"), "$ x^(n + 1)_i $");
        assert_eq!(readable("f(x, y)"), "$ f(x, y) $");
        assert_eq!(readable("sin(x)"), "$ sin (x) $");
        assert_eq!(readable("lim_(n -> oo) a"), "$ lim_(n → ∞) a $");
        assert_eq!(readable("(a b)^2"), "$ (a b)^2 $");
        assert_eq!(readable("/"), "$ #sem-slot()/#sem-slot() $");
    }

    /// The readable style must render exactly like the edit style: compare
    /// pixels for many formulas, including every state while typing them.
    #[test]
    fn readable_renders_like_edit() {
        let mut world = BridgeWorld::new();
        let formulas = [
            "S <= (log chi)/(2) + c",
            "a b/c d + 3 1/2 + e^x/2 + 1/2/3",
            "x^12 + x^1.5 + x_(i j)^2 + x' + f'(x)",
            "f(x, y) + g (x) + [a, b) + {1; 2}",
            "lim_(n -> oo) a_n + max_(x in A) f + liminf x",
            "(a+b)^2 + (x, y)^2_1 + sum_(i=1)^n i",
            "alpha/beta + oo/2 + (-1)/2 + -1/2 + 2/(-x)",
            "e^(-x^2/2) + \\x + 1.5/2.5",
            "| x | + (a | b) + a, b; c",
            "/ + x^ + (/)/ + ^/_",
        ];
        let render = |world: &mut BridgeWorld, snippet: &str| {
            let (src, _) = block_document(&Around::default(), snippet, 400.0);
            let doc = compile(world, &src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
            crate::render_page(&doc.pages()[0], 2.0).data
        };
        for keys in formulas {
            let mut d = Document::empty();
            let b = d.display(vec![]);
            d.blocks = vec![b];
            let mut e = model::Editor::new(d);
            e.set_symbol_resolver(crate::resolve_symbol);
            for c in keys.chars() {
                e.insert_text(&c.to_string());
                let block = &e.doc().blocks[0];
                let edit = serialize_block_styled(block, Style::Edit).0;
                let read = serialize_block_styled(block, Style::Readable).0;
                if edit == read {
                    continue;
                }
                assert!(
                    render(&mut world, &edit) == render(&mut world, &read),
                    "renders differ after {c:?} in {keys:?}:\n  edit:     {edit}\n  readable: {read}"
                );
            }
        }
    }

    #[test]
    fn diagrams_serialize_and_compile() {
        assert!(CETZ_IMPORT.contains(CETZ_VERSION));
        let mut d = Document::empty();
        let t = d.text("Before");
        let p = d.paragraph(vec![t]);
        let g = d.diagram("circle((0, 0))\nline((0, 0), (1, 1))");
        let gid = g.id;
        d.blocks = vec![p, g.clone()];
        let (snippet, map) = serialize_block(&g);
        assert_eq!(
            snippet,
            format!(
                "{CETZ_IMPORT}\n#align(center, cetz.canvas({{\n  import cetz.draw: *\n  \
                 circle((0, 0))\n  line((0, 0), (1, 1))\n}}))"
            )
        );
        // The span map finds every char of the drawing code.
        let at = map.text_byte_of(gid, 15).unwrap();
        assert_eq!(&snippet[at..at + 4], "line");
        let doc = serialize_document(&d);
        assert_eq!(doc.matches(CETZ_IMPORT).count(), 1, "{doc}");
        let mut world = BridgeWorld::new();
        let (src, _) = block_document(&Around::default(), &snippet, 300.0);
        compile(&mut world, &src).unwrap();
        compile(&mut world, &doc).unwrap();
    }

    /// Lists are Typst markup: item bodies indented past the marker, a
    /// tight list one item per line, nesting by indentation. Compiled, the
    /// structure is what the model says.
    #[test]
    fn lists() {
        let mut d = Document::empty();
        let p = |d: &mut Document, s: &str| {
            let t = d.text(s);
            d.paragraph(vec![t])
        };
        let (a, b, c, e) = (
            p(&mut d, "a"),
            p(&mut d, "b"),
            p(&mut d, "c"),
            p(&mut d, "e"),
        );
        let x = d.ident("x");
        let math = d.display(vec![x]);
        let nested = d.item(ListKind::Numbered, vec![b]);
        let first = d.item(ListKind::Bullet, vec![a, nested, c, math]);
        let second = d.item(ListKind::Bullet, vec![e]);
        let (t, desc) = (d.text("T"), p(&mut d, "desc"));
        let mut term = d.item(ListKind::Term, vec![desc]);
        if let Block::Item { term: tt, .. } = &mut term.kind {
            *tt = vec![t];
        }
        d.blocks = vec![first, second, term];
        let doc = serialize_document(&d);
        assert!(
            doc.ends_with("\n- a\n  + b\n\n  c\n\n  $ x $\n- e\n/ T: desc\n"),
            "{doc}"
        );
        use typst::foundations::{Element, Selector};
        use typst::introspection::Introspector;
        use typst::model::{EnumElem, ListElem};
        let mut world = BridgeWorld::new();
        let compiled = compile(&mut world, &doc).unwrap();
        let lists = compiled
            .introspector()
            .query(&Selector::Elem(Element::of::<ListElem>(), None));
        assert_eq!(lists.len(), 1, "one bullet list (tight, two items)");
        let enums = compiled
            .introspector()
            .query(&Selector::Elem(Element::of::<EnumElem>(), None));
        assert_eq!(enums.len(), 1);

        // Rendered on its own, a numbered item shows its place in the list.
        let one = p(&mut d, "third");
        let item = d.item(ListKind::Numbered, vec![one]);
        assert_eq!(serialize_block_numbered(&item, Some(3)).0, "3. third");
        assert_eq!(serialize_block_styled(&item, Style::Readable).0, "+ third");
    }

    /// A call being typed shows in a code font, at the start of a
    /// paragraph as in the middle of it, only in the editor's renders; other
    /// `#`s do not.
    #[test]
    fn calls_being_typed_in_code_font() {
        let mut d = Document::empty();
        for (s, code) in [
            ("#", true),
            ("#ali", true),
            ("#align(cen", true),
            ("#align(center)", true),
            ("#1", false),
            ("#(x", false),
            ("# x", false),
        ] {
            let t = d.text(s);
            let p = d.paragraph(vec![t]);
            let (src, _) = serialize_block(&p);
            assert_eq!(src.starts_with(CODE_TEXT), code, "{s:?}: {src}");
            assert!(
                !serialize_block_styled(&p, Style::Readable)
                    .0
                    .contains("#text")
            );
            let mut world = BridgeWorld::new();
            let (doc, _) = block_document(&Around::default(), &src, 300.0);
            compile(&mut world, &doc).unwrap();
        }
        // In the middle of a text: only the call, up to its `)`.
        for (s, code) in [
            ("a #b c", Some("\\#b]")),
            ("x#fn(a, (b)) y", Some("\\#fn(a, (b))]")),
            ("end #", Some("\\#]")),
            ("a #1 b", None),
            ("a # b", None),
        ] {
            let t = d.text(s);
            let p = d.paragraph(vec![t]);
            let (src, _) = serialize_block(&p);
            match code {
                Some(code) => assert!(src.contains(&format!("{CODE_TEXT}{code}")), "{s:?}: {src}"),
                None => assert!(!src.contains(CODE_TEXT), "{s:?}: {src}"),
            }
            let mut world = BridgeWorld::new();
            let (doc, _) = block_document(&Around::default(), &src, 300.0);
            compile(&mut world, &doc).unwrap();
        }
    }

    /// Calls in math: `name(arg, …)`; a comma inside an argument is
    /// `comma`, so it cannot split the arguments. Every argument gets a box
    /// (the caret moves through them), the call one around its drawing.
    #[test]
    fn math_calls() {
        let mut d = Document::empty();
        let (three, y, a, b) = (d.num("3"), d.ident("y"), d.ident("a"), d.ident("b"));
        let comma = d.sym(",");
        let (index, radicand) = (d.row(vec![three]), d.row(vec![y]));
        let (iid, rid) = (index.id, radicand.id);
        let root = d.call("root", vec![index, radicand]);
        let inner = d.row(vec![a, comma, b]);
        let abs = d.call("abs", vec![inner]);
        let root_id = root.id;
        let block = d.display(vec![root, abs]);
        let (readable, _) = serialize_block_styled(&block, Style::Readable);
        assert_eq!(readable, "$ root(3, y) abs(a comma b) $");
        let (src, map) = serialize_block(&block);
        let mut world = BridgeWorld::new();
        let (doc, offset) = block_document(&Around::default(), &src, 300.0);
        let compiled = compile(&mut world, &doc).unwrap();
        let l = crate::raw_layout(&world, &compiled.pages()[0], offset).resolve(&map);
        let (ri, rr, rc) = (
            l.box_of(iid).unwrap(),
            l.box_of(rid).unwrap(),
            l.box_of(root_id).unwrap(),
        );
        assert!(
            ri.right() <= rr.x && ri.h < rr.h,
            "small index before: {ri:?} {rr:?}"
        );
        assert!(rc.x < ri.x + 1.0 && rc.right() >= rr.right(), "{rc:?}");
    }

    /// Atoms: primes right after their base (else they would not attach),
    /// text quoted, `&` and `\` as in Typst (`\` always followed by a
    /// space, so it cannot escape what follows).
    #[test]
    fn math_atoms() {
        let mut d = Document::empty();
        let (f, x, a, b, c) = (
            d.ident("f"),
            d.ident("x"),
            d.ident("a"),
            d.ident("b"),
            d.ident("c"),
        );
        let primes = d.node(MathKind::Primes(2));
        let (open, close, comma) = (d.sym("("), d.sym(")"), d.sym(","));
        let text = d.node(MathKind::Text("if \"x\"".into()));
        let (align, br, eq) = (
            d.node(MathKind::Align),
            d.node(MathKind::Linebreak),
            d.sym("="),
        );
        // Spaces around text show: kept where the tree has them.
        let (mut text, mut a) = (text, a);
        (text.space, a.space) = (true, true);
        let block = d.display(vec![
            f, primes, open, x, close, text, a, align, eq, b, br, comma, c,
        ]);
        let (readable, _) = serialize_block_styled(&block, Style::Readable);
        assert_eq!(readable, "$ f'' (x) \"if \\\"x\\\"\" a & = b \\ , c $");
        let (edit, _) = serialize_block(&block);
        assert!(edit.contains("f'' ("), "{edit}");
        let mut world = BridgeWorld::new();
        for src in [readable, edit] {
            let (doc, _) = block_document(&Around::default(), &src, 300.0);
            compile(&mut world, &doc).unwrap();
            fn attached(n: &typst::syntax::SyntaxNode) -> bool {
                n.kind() == typst::syntax::SyntaxKind::MathAttach || n.children().any(attached)
            }
            assert!(
                attached(&typst::syntax::parse(&src)),
                "the primes attach to f: {src}"
            );
        }
    }

    /// An inline diagram: its drawing in a box and canvas in the file, boxed
    /// again in the editor's renders so it gets a layout box (CeTZ shapes
    /// carry CeTZ's spans), without moving what follows. CeTZ is imported
    /// where it is needed; the preview shows the diagram alone.
    #[test]
    fn inline_diagrams() {
        let mut d = Document::empty();
        let (a, b) = (d.ident("a"), d.ident("b"));
        let code = d.inline_diagram(model::INLINE_DIAGRAM_TEMPLATE);
        let (aid, cid, bid) = (a.id, code.id, b.id);
        let semi = d.sym(";");
        let m = d.math(vec![a, code, semi, b]);
        let p = d.paragraph(vec![m]);
        let (readable, _) = serialize_block_styled(&p, Style::Readable);
        assert_eq!(
            readable,
            "$a #box(baseline: 30%, cetz.canvas(length: 1em, {\n  import cetz.draw: *\n  \
             circle((0, 0), radius: 0.5)\n  line((-0.5, 0), (0.5, 0))\n})) ; b$",
            "space kept before ;"
        );
        let (src, map) = serialize_block(&p);
        assert!(src.starts_with(CETZ_IMPORT), "{src}");
        let mut world = BridgeWorld::new();
        let (doc, offset) = block_document(&Around::default(), &src, 300.0);
        let compiled = compile(&mut world, &doc).unwrap();
        let l = crate::raw_layout(&world, &compiled.pages()[0], offset).resolve(&map);
        let (ra, rc, rb) = (
            l.box_of(aid).unwrap(),
            l.box_of(cid).expect("the code has a box"),
            l.box_of(bid).unwrap(),
        );
        assert!(
            ra.right() <= rc.x && rc.right() <= rb.x && rc.w > 10.0,
            "{ra:?} {rc:?} {rb:?}"
        );

        // The preview shows the diagram alone.
        let alone = serialize_math_node(&p.rows()[0].nodes[1]);
        assert!(
            alone.starts_with(CETZ_IMPORT) && !alone.contains("$a"),
            "{alone}"
        );
        compile(&mut world, &alone).unwrap();

        d.blocks = vec![p];
        let whole = serialize_document(&d);
        assert_eq!(whole.matches(CETZ_IMPORT).count(), 1, "{whole}");
        compile(&mut world, &whole).unwrap();
    }

    /// A block call's source arguments come before its title and body,
    /// verbatim, and compile.
    #[test]
    fn block_call_arguments() {
        let mut d = Document::empty();
        let t = d.text("Centred");
        let body = d.paragraph(vec![t]);
        let mut call = d.env("align", vec![body]);
        if let Block::Env { args, .. } = &mut call.kind {
            *args = "center + horizon".into();
        }
        let (src, _) = serialize_block(&call);
        assert_eq!(src, "#align(center + horizon)[\nCentred\n]");
        let mut world = BridgeWorld::new();
        compile(&mut world, &src).unwrap();
    }

    #[test]
    fn span_map_ranges() {
        let mut d = Document::empty();
        let a = d.ident("a");
        let b = d.ident("b");
        let (aid, bid) = (a.id, b.id);
        let f = d.frac(vec![a], vec![b]);
        let fid = f.id;
        let MathKind::Frac { num, den } = &f.kind else {
            unreachable!()
        };
        let (num_id, den_id) = (num.id, den.id);
        let block = d.display(vec![f]);
        let (src, map) = serialize_block(&block);
        assert_eq!(src, "$ (a)/(b) $");
        assert_eq!(&src[map.range(fid).unwrap()], "(a)/(b)");
        assert_eq!(&src[map.range(aid).unwrap()], "a");
        assert_eq!(map.range(num_id), map.range(aid));
        assert_eq!(map.innermost(src.find('b').unwrap()), Some(bid));
        assert_eq!(map.innermost(src.find('/').unwrap()), Some(fid));
        assert_eq!(map.range(block.id), Some(0..src.len()));
        assert!(map.range(den_id).is_some());
    }

    #[test]
    fn text_char_mapping_skips_escapes() {
        let mut d = Document::empty();
        let t = d.text("a*b");
        let tid = t.id;
        let block = d.paragraph(vec![t]);
        let (src, map) = serialize_block(&block);
        assert_eq!(src, "a\\*b");
        assert_eq!(map.text_char_at(tid, 0), Some(0));
        assert_eq!(map.text_char_at(tid, 1), Some(1));
        assert_eq!(map.text_char_at(tid, 2), Some(1));
        assert_eq!(map.text_char_at(tid, 3), Some(2));
        assert_eq!(map.text_byte_of(tid, 3), Some(4));
    }

    #[test]
    fn document_export() {
        let mut d = Document::empty();
        let t = d.text("Hi");
        let p = d.paragraph(vec![t]);
        let x = d.ident("x");
        let m = d.display(vec![x]);
        d.blocks = vec![p, m];
        let typ = serialize_document(&d);
        assert_eq!(typ, format!("{PREAMBLE}\nHi\n\n$ x $\n"));
        let mut world = BridgeWorld::new();
        compile(&mut world, &typ).unwrap();
    }

    /// A figure's caption is its `caption:` argument; an empty one only
    /// shows in the editor. A labelled figure is referenced as "Figure 1".
    /// A title block renders alone (setting the metadata itself), and the
    /// export sets the document's metadata: title, authors and date.
    #[test]
    fn title_block() {
        use typst::model::Document as _;
        let mut d = Document::empty();
        let fields = model::TitleFields {
            title: "On $x^2$".into(),
            authors: vec!["Ann".into()],
            date: "2026-11-10".into(),
        };
        let title = d.title_block(&fields.source());
        let t = d.text("Body.");
        let p = d.paragraph(vec![t]);
        d.blocks = vec![title, p];
        let mut world = BridgeWorld::new();
        let (snippet, _) = serialize_block(&d.blocks[0]);
        let (src, _) = block_document(&Around::default(), &snippet, 300.0);
        compile(&mut world, &src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));

        let typ = serialize_document(&d);
        assert!(typ.contains(
            "#set document(title: [On $x^2$], author: (\"Ann\",), \
             date: datetime(year: 2026, month: 11, day: 10))\n"
        ));
        assert!(typ.contains("\n#title()\n\n#context document.author.join(\", \")\n"));
        let doc = compile(&mut world, &typ).unwrap();
        let info = doc.info();
        assert_eq!(info.title.as_deref(), Some("On x2"));
        assert_eq!(info.author, ["Ann"]);
    }

    #[test]
    fn figures() {
        let mut e = model::Editor::new(Document::new());
        e.insert_text("#figure Body");
        e.move_right();
        e.insert_text("A *bold* caption");
        let mut d = e.doc().clone();
        d.blocks[0].label = "fig".into();
        let figure = &d.blocks[0];
        assert_eq!(
            ser(figure),
            "#figure(caption: [A #strong[bold] caption])[\nBody\n] <fig>"
        );
        assert_eq!(
            serialize_block_styled(figure, Style::Readable).0,
            "#figure(caption: [A *bold* caption])[\nBody\n] <fig>"
        );
        let refs = d.raw("See @fig.");
        d.blocks.push(refs);
        let typ = serialize_document(&d);
        let mut world = BridgeWorld::new();
        compile(&mut world, &typ).unwrap_or_else(|e| panic!("{e:?}\n{typ}"));
        // A new figure's empty caption.
        let mut e = model::Editor::new(Document::new());
        e.insert_text("#figure x");
        let figure = &e.doc().blocks[0];
        assert_eq!(
            ser(figure),
            "#figure(caption: [#text(fill: rgb(0, 0, 0, 0))[x]])[\nx\n]"
        );
        assert_eq!(
            serialize_block_styled(figure, Style::Readable).0,
            "#figure[\nx\n]"
        );
        // A diagram in a figure renders in the editor.
        let mut d = Document::empty();
        let diagram = d.diagram("circle((0, 0))");
        let cap = d.text("Cap");
        let cap = d.caption(vec![cap]);
        let figure = d.env("figure", vec![diagram, cap]);
        let (src, _) = block_document(&Around::default(), &ser(&figure), 300.0);
        compile(&mut world, &src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    }

    /// Labels follow what they label; references to them resolve.
    #[test]
    fn labels() {
        let mut d = Document::empty();
        d.header = "#set heading(numbering: \"1.1\")\n#set math.equation(numbering: \"(1)\")\n#let lemma(body) = figure(kind: \"lemma\", supplement: [Lemma], body)\n".into();
        let t = d.text("Intro");
        let mut h = d.heading(1, vec![t]);
        h.label = "sec:intro".into();
        let x = d.ident("x");
        let mut m = d.display(vec![x]);
        m.label = "eq:x".into();
        let t = d.text("Claim.");
        let p = d.paragraph(vec![t]);
        let mut env = d.env("lemma", vec![p]);
        env.label = "lem".into();
        let refs = d.raw("See @sec:intro, @eq:x and @lem.");
        // References in text; `.5` after one would continue its label.
        let see = d.text("Also ");
        let r1 = d.reference("sec:intro");
        let and = d.text(" and ");
        let r2 = d.reference("lem");
        let tail = d.text(".5 times.");
        let para = d.paragraph(vec![see, r1, and, r2, tail]);
        assert_eq!(
            serialize_block_styled(&para, Style::Readable).0,
            "Also @sec:intro and #ref(<lem>).5 times."
        );
        assert_eq!(
            ser(&para),
            "Also #sem-code[@sec:intro] and #sem-code[@lem].5 times."
        );
        d.blocks = vec![h, m, env, refs, para];
        let typ = serialize_document(&d);
        assert!(typ.contains("= Intro <sec:intro>\n"), "{typ}");
        assert!(typ.contains("$ x $ <eq:x>\n"), "{typ}");
        assert!(typ.contains("#lemma[\nClaim.\n] <lem>\n"), "{typ}");
        assert_eq!(ser(&d.blocks[1]), "$ x $ <eq:x>");
        let mut world = BridgeWorld::new();
        compile(&mut world, &typ).unwrap_or_else(|e| panic!("{e:?}\n{typ}"));
    }
}

#[cfg(test)]
mod inline_call_tests {
    use super::*;
    use crate::{Around, BridgeWorld, block_document, compile};

    fn typed_text(keys: &str) -> model::Editor {
        let mut e = model::Editor::new(Document::new());
        e.insert_text(keys);
        e
    }

    fn both(keys: &str) -> (String, String) {
        let e = typed_text(keys);
        let block = &e.doc().blocks[0];
        (
            serialize_block_styled(block, Style::Edit).0,
            serialize_block_styled(block, Style::Readable).0,
        )
    }

    #[test]
    fn inline_calls_serialize() {
        let cases = [
            ("a *b* c", "a #strong[b] c", "a *b* c"),
            ("_i_, *b*.", "#emph[i], #strong[b];.", "_i_, *b*."),
            ("x *y", "x #strong[y]", "x *y*"),
            (
                "*a _b *c* d_ e*",
                "#strong[a #emph[b #strong[c] d] e]",
                "*a _b #strong[c] d_ e*",
            ),
            ("*b *", "#strong[b ]", "#strong[b ]"),
            ("*$x$*", "#strong[$x$]", "*$x$*"),
            (
                "see #link(\"https://x.org\")[here](1)",
                "see #link(\"https://x.org\")[here];(1)",
                "see #link(\"https://x.org\")[here];(1)",
            ),
            ("x #cl[a]; b", "x #cl[a];; b", "x #cl[a];; b"),
            (
                "x #cl[",
                "x #cl[#text(fill: rgb(0, 0, 0, 0))[x]]",
                "x #cl[]",
            ),
        ];
        for (keys, edit, readable) in cases {
            assert_eq!(both(keys), (edit.into(), readable.into()), "{keys}");
        }
    }

    /// Every state while typing compiles, and the export renders like the
    /// editor's render.
    #[test]
    fn inline_calls_compile_and_export_the_same() {
        let mut world = BridgeWorld::new();
        let mut render = |snippet: &str| {
            let (src, _) = block_document(&Around::default(), snippet, 400.0);
            let doc = compile(&mut world, &src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
            crate::render_page(&doc.pages()[0], 2.0).data
        };
        let texts = [
            "a *bold* and _it_ (*x*) *y*, _z_.",
            "*a _b *c* d_ e* x#strong[y]z",
            "see #link(\"https://typst.app\")[the _site_]. #underline[u](1)",
            "*$x^2$ is* _$y$_",
            "run `ls -l` and `a\"b` -- or --- x~y z\\ next\\ \\ end",
            "`code` *`in bold`*",
        ];
        for keys in texts {
            let mut e = model::Editor::new(Document::new());
            for c in keys.chars() {
                e.insert_text(&c.to_string());
                let block = &e.doc().blocks[0];
                render(&serialize_block_styled(block, Style::Edit).0);
            }
            let block = &e.doc().blocks[0];
            let edit = serialize_block_styled(block, Style::Edit).0;
            let read = serialize_block_styled(block, Style::Readable).0;
            assert!(
                render(&edit) == render(&read),
                "renders differ for {keys:?}:\n  edit:     {edit}\n  readable: {read}"
            );
        }
    }

    /// Code in a formula is a box in the editor; no space may show next
    /// to it (the PEPS definition's `#sym.brace.l h` had a gap).
    #[test]
    fn code_in_formula_renders_tight() {
        let mut world = BridgeWorld::new();
        let mut render = |snippet: &str| {
            let (src, _) = block_document(&Around::default(), snippet, 400.0);
            let doc = compile(&mut world, &src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
            crate::render_page(&doc.pages()[0], 2.0).data
        };
        let texts = [
            "where $e = #sym.brace.l h, overline(h) #sym.brace.r$,",
            "$x #sym.brace.l (b) #sym.brace.r . c$",
        ];
        for text in texts {
            let d = crate::import_typ(text);
            let block = &d.blocks[0];
            let edit = serialize_block_styled(block, Style::Edit).0;
            let read = serialize_block_styled(block, Style::Readable).0;
            assert!(
                render(&edit) == render(&read),
                "renders differ for {text:?}:\n  edit:     {edit}\n  readable: {read}"
            );
        }
    }
}
