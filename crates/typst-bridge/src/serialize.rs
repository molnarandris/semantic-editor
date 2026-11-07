//! Tree → Typst source. The tree is the source of truth; Typst source is
//! always derived. Each block serializes to a standalone snippet plus a
//! [`SpanMap`] from node ids to byte ranges in that snippet.

use std::ops::Range;

use model::{Block, BlockNode, Document, Inline, InlineNode, MathKind, MathNode, MathRow, NodeId};

/// Shared definitions and styles. Prepended to every block document and to
/// the exported `.typ` file.
///
/// `sem-slot` must stay a `box.with(..)`: then the box element is created at
/// the call site and its frame carries the slot's own span. A closure
/// (`#let sem-slot() = box(..)`) would give every slot the preamble's span.
pub const PREAMBLE: &str = "\
#set text(font: \"New Computer Modern\", size: 11pt)
#show math.equation: set text(font: \"New Computer Modern Math\")
#let sem-slot = box.with(width: 0.55em, height: 0.7em, stroke: (paint: luma(160), thickness: 0.5pt, dash: \"densely-dashed\"))
";

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

/// The Typst source of an empty math row (a visible placeholder box).
pub const SLOT: &str = "#sem-slot()";

/// Typst's predefined math operators (`typst-library` `math/op.rs`): name,
/// displayed text (`liminf` has a thin space, U+2009), and whether
/// attachments go above/below in display math.
///
/// They are emitted as `op(#[log])` rather than `log`: the predefined ones
/// take their text from the library, so their glyphs carry library spans and
/// could not be mapped back to the node. `op("log")` loses the span too (a
/// string argument is a value); a markup block keeps it. Same element, same
/// rendering.
const OPERATORS: &[(&str, &str, bool)] = &[
    ("arccos", "arccos", false),
    ("arcsin", "arcsin", false),
    ("arctan", "arctan", false),
    ("arg", "arg", false),
    ("cos", "cos", false),
    ("cosh", "cosh", false),
    ("cot", "cot", false),
    ("coth", "coth", false),
    ("csc", "csc", false),
    ("csch", "csch", false),
    ("ctg", "ctg", false),
    ("deg", "deg", false),
    ("det", "det", true),
    ("dim", "dim", false),
    ("exp", "exp", false),
    ("gcd", "gcd", true),
    ("lcm", "lcm", true),
    ("hom", "hom", false),
    ("id", "id", false),
    ("im", "im", false),
    ("inf", "inf", true),
    ("ker", "ker", false),
    ("lg", "lg", false),
    ("lim", "lim", true),
    ("liminf", "lim\u{2009}inf", true),
    ("limsup", "lim\u{2009}sup", true),
    ("ln", "ln", false),
    ("log", "log", false),
    ("max", "max", true),
    ("min", "min", true),
    ("mod", "mod", false),
    ("Pr", "Pr", true),
    ("sec", "sec", false),
    ("sech", "sech", false),
    ("sin", "sin", false),
    ("sinc", "sinc", false),
    ("sinh", "sinh", false),
    ("sup", "sup", true),
    ("tan", "tan", false),
    ("tanh", "tanh", false),
    ("tg", "tg", false),
    ("tr", "tr", false),
];

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

    fn block(&mut self, block: &BlockNode) {
        self.node(block.id, |w| match &block.kind {
            Block::Heading { level, inlines } => {
                for _ in 0..(*level).max(1) {
                    w.out.push('=');
                }
                w.out.push(' ');
                w.inlines(inlines, false);
            }
            Block::Paragraph(inlines) => w.inlines(inlines, true),
            Block::DisplayMath(row) => {
                w.out.push_str("$ ");
                w.row(row);
                w.out.push_str(" $");
            }
            Block::RawTypst(src) => w.verbatim(block.id, src, ""),
            Block::Diagram(src) => {
                if w.style == Style::Edit {
                    // Standalone block documents import CeTZ themselves; the
                    // whole document imports it once at the top.
                    w.out.push_str(CETZ_IMPORT);
                    w.out.push('\n');
                }
                w.out
                    .push_str("#align(center, cetz.canvas({\n  import cetz.draw: *\n  ");
                w.verbatim(block.id, src, "  ");
                w.out.push_str("\n}))");
            }
        });
    }

    /// Typst source copied as is, each line after the first indented by
    /// `indent`; records where every char went.
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

    fn inlines(&mut self, inlines: &[InlineNode], at_line_start: bool) {
        for (i, inline) in inlines.iter().enumerate() {
            let line_start = at_line_start && i == 0;
            self.node(inline.id, |w| match &inline.kind {
                Inline::Text(s) => w.text(inline.id, s, line_start),
                Inline::Math(row) => {
                    w.out.push('$');
                    w.row(row);
                    w.out.push('$');
                }
            });
        }
    }

    fn text(&mut self, id: NodeId, s: &str, line_start: bool) {
        let mut starts = Vec::with_capacity(s.len() + 1);
        let chars: Vec<char> = s.chars().collect();
        // "1." at the start of a line would become a numbered list.
        let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
        let enum_dot = line_start && digits > 0 && chars.get(digits) == Some(&'.');
        for (i, &c) in chars.iter().enumerate() {
            starts.push(self.out.len());
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
                '\n' | '\r' => self.out.push(' '),
                _ if escape => {
                    self.out.push('\\');
                    self.out.push(c);
                }
                _ => self.out.push(c),
            }
        }
        starts.push(self.out.len());
        self.map.texts.push((id, starts));
    }

    fn row(&mut self, row: &MathRow) {
        self.node(row.id, |w| {
            if row.nodes.is_empty() {
                w.out.push_str(SLOT);
            }
            let paired = paired_delimiters(&row.nodes);
            for (i, node) in row.nodes.iter().enumerate() {
                if i > 0 && w.space_between(&row.nodes[i - 1], node, paired[i - 1], paired[i]) {
                    w.out.push(' ');
                }
                match (&node.kind, paired[i]) {
                    // Literal, so Typst scales the pair to its content.
                    (MathKind::Sym(d), true) => w.node(node.id, |w| w.out.push_str(d)),
                    _ => w.math(node),
                }
            }
        });
    }

    /// Whether to put a space between two neighbouring nodes. Spaces between
    /// atoms do not change Typst's output; the readable style drops them
    /// only next to brackets and punctuation, where no identifier can form.
    fn space_between(&self, a: &MathNode, b: &MathNode, a_paired: bool, b_paired: bool) -> bool {
        if self.style == Style::Edit {
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

fn quoted(s: &str) -> String {
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
        '|' => "bar.v",
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
    let mut w = Writer {
        out: String::new(),
        map: SpanMap::default(),
        style,
        args: 0,
    };
    w.block(block);
    (w.out, w.map)
}

/// The whole document as one readable Typst file: preamble + blocks.
pub fn serialize_document(doc: &Document) -> String {
    serialize_document_mapped(doc).0
}

/// The whole document as one readable Typst file, with a span map over all
/// blocks.
pub fn serialize_document_mapped(doc: &Document) -> (String, SpanMap) {
    let mut out = String::from(PREAMBLE);
    if doc
        .blocks
        .iter()
        .any(|b| matches!(b.kind, Block::Diagram(_)))
    {
        out.push_str(CETZ_IMPORT);
        out.push('\n');
    }
    let mut map = SpanMap::default();
    for block in &doc.blocks {
        out.push('\n');
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
    use crate::{BridgeWorld, block_document, compile};

    fn ser(block: &BlockNode) -> String {
        serialize_block(block).0
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
            "$ comma semi ( ) bar.v slash amp \\# \\$ → \\@ $",
        ));

        let t = d.text("1. Use *bold* and _x_ #code $5 @ref <l> [a] a~b // c -- d -? e/f - g");
        cases.push((
            "text escapes",
            d.paragraph(vec![t]),
            "1\\. Use \\*bold\\* and \\_x\\_ \\#code \\$5 \\@ref \\<l> \\[a\\] a\\~b \\// c \\-- d \\-? e/f - g",
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
            let (src, _) = block_document(&ser(&block), 300.0);
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
            "\\alpha^2_i/x + \\foo ' , ; | [ ] { } & ~~ := \\",
            "(a+b)^2 / ( ) _ ^ 3.14.15 é ∑ #$",
            "e^(-x^2/2) ! != -> <-> 1/2/3",
            "a times.o b arrow.r.long c eq.not x. times.q y. lt.eq.slant",
        ];
        let mut d = Document::empty();
        let p = d.paragraph(vec![]);
        d.blocks = vec![p];
        for keys in sessions {
            let mut e = model::Editor::new(d.clone());
            e.set_symbol_resolver(crate::resolve_symbol);
            e.insert_text("text $");
            for c in keys.chars() {
                e.insert_text(&c.to_string());
                let block = &e.doc().blocks[0];
                let (src, _) = block_document(&ser(block), 300.0);
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
            "e^(-x^2/2) + sqrt + \\foo + 1.5/2.5",
            "| x | + (a | b) + a, b; c",
            "/ + x^ + (/)/ + ^/_",
        ];
        let render = |world: &mut BridgeWorld, snippet: &str| {
            let (src, _) = block_document(snippet, 400.0);
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
        let (src, _) = block_document(&snippet, 300.0);
        compile(&mut world, &src).unwrap();
        compile(&mut world, &doc).unwrap();
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
}
