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

/// The Typst source of an empty math row (a visible placeholder box).
pub const SLOT: &str = "#sem-slot()";

/// Typst's predefined math operators (`typst-library` `math/op.rs`): name,
/// displayed text, and whether attachments go above/below in display math.
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
    ("liminf", "lim inf", true),
    ("limsup", "lim sup", true),
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

struct Writer {
    out: String,
    map: SpanMap,
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
            Block::RawTypst(src) => w.out.push_str(src),
        });
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
                if i > 0 {
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

    fn math(&mut self, node: &MathNode) {
        self.node(node.id, |w| match &node.kind {
            MathKind::Sym(s) => w.out.push_str(&sym_source(s)),
            MathKind::Ident(s) => w.out.push_str(&ident_source(s)),
            MathKind::Num(s) => w.out.push_str(&num_source(s)),
            MathKind::Slot => w.out.push_str(SLOT),
            MathKind::Frac { num, den } => {
                w.out.push('(');
                w.row(num);
                w.out.push_str(")/(");
                w.row(den);
                w.out.push(')');
            }
            MathKind::Scripts { base, sup, sub } => {
                if is_simple_base(base) {
                    w.row(base);
                    if let Some(sup) = sup {
                        w.out.push_str("^(");
                        w.row(sup);
                        w.out.push(')');
                    }
                    if let Some(sub) = sub {
                        w.out.push_str("_(");
                        w.row(sub);
                        w.out.push(')');
                    }
                } else {
                    // `(a b)^2` would show the parentheses; attach() does not.
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

fn ident_source(s: &str) -> String {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_alphabetic() => s.into(),
        _ => match OPERATORS.iter().find(|(name, ..)| *name == s) {
            Some((_, text, true)) => format!("op(#[{text}], limits: #true)"),
            Some((_, text, false)) => format!("op(#[{text}])"),
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
    let mut w = Writer {
        out: String::new(),
        map: SpanMap::default(),
    };
    w.block(block);
    (w.out, w.map)
}

/// The whole document as one Typst file: preamble + blocks.
pub fn serialize_document(doc: &Document) -> String {
    serialize_document_mapped(doc).0
}

/// The whole document as one Typst file, with a span map over all blocks
/// (for the source pane).
pub fn serialize_document_mapped(doc: &Document) -> (String, SpanMap) {
    let mut out = String::from(PREAMBLE);
    let mut map = SpanMap::default();
    for block in &doc.blocks {
        out.push('\n');
        let (snippet, mut block_map) = serialize_block(block);
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
