//! The document tree. Every node carries a stable [`NodeId`] that is never
//! reused within a document. Blocks' ids are saved, so remarks can point at
//! a block and versions can be compared block by block; the other nodes'
//! ids are not, they are regenerated on load.

use serde::{Deserialize, Serialize};

use crate::cursor::Step;

#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Not assigned yet (a node read from a file without its id).
    pub fn is_unset(&self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockNode {
    /// Saved (see [`Document::restore_ids`]); missing in older files.
    #[serde(default, skip_serializing_if = "NodeId::is_unset")]
    pub id: NodeId,
    #[serde(flatten)]
    pub kind: Block,
    /// The block's label (`<label>` after it), empty if none. Used on
    /// headings, display formulas and environments (see
    /// [`Block::labelable`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Heading {
        level: u8,
        inlines: Vec<InlineNode>,
    },
    Paragraph(Vec<InlineNode>),
    DisplayMath(MathRow),
    RawTypst(String),
    /// A CeTZ drawing: the body of `cetz.canvas({ import cetz.draw: * … })`.
    Diagram(String),
    /// A call of a document function with a body of blocks, `#lemma[…]`:
    /// theorem-like environments, proofs, … The function (from the header or
    /// a code block) adds the label, number and formatting; the body is
    /// edited in place. The body is never empty. The optional title
    /// (`#lemma[Title][…]`, as in ctheorems and theorion) is empty when there
    /// is none; otherwise it alternates like a paragraph's inlines. `args`
    /// are the call's other arguments as Typst source, kept verbatim
    /// (`center` in `#align(center)[…]`); empty if none.
    Env {
        name: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        args: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        title: Vec<InlineNode>,
        body: Vec<BlockNode>,
    },
    /// A list item, `- …`, `+ …` or `/ Term: …`. As in Typst, consecutive
    /// items make a list and there is no list node; an item inside an item's
    /// body is nested. The body is never empty. `term` is used by term items
    /// only (alternating like a paragraph's inlines; empty for the others).
    Item {
        kind: ListKind,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        term: Vec<InlineNode>,
        body: Vec<BlockNode>,
    },
    /// The caption of a figure: the last block of its body, edited in place
    /// under it, written as the call's `caption:` argument.
    Caption(Vec<InlineNode>),
    /// The bibliography: its BibTeX (or Hayagriva YAML) source, embedded and
    /// edited in the source dialog. Typst lists the works the document cites.
    Bibliography(String),
    /// The title block: the document's title, authors and date
    /// ([`TitleFields`], stored as lines), shown where the block is (at the
    /// top) and set as the document's metadata. Edited in a dialog.
    Title(String),
    /// A picture, `#image("images/x.png", width: 80%)`: the path of a
    /// project file and the other arguments as Typst code (empty if none),
    /// typed in an options tab like a math call's. The body of a figure.
    Image {
        path: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        opts: String,
    },
}

/// The fields of a title block. The title is Typst markup; the date is
/// `YYYY-MM-DD` or empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleFields {
    pub title: String,
    pub authors: Vec<String>,
    pub date: String,
}

impl TitleFields {
    /// Read a title block's source: the title, the date, then one author
    /// per line.
    pub fn parse(source: &str) -> Self {
        let mut lines = source.split('\n');
        let title = lines.next().unwrap_or_default().to_string();
        let date = lines.next().unwrap_or_default().to_string();
        let authors = lines
            .filter(|l| !l.trim().is_empty())
            .map(String::from)
            .collect();
        TitleFields {
            title,
            authors,
            date,
        }
    }

    /// The source of a title block with these fields.
    pub fn source(&self) -> String {
        let mut lines = vec![self.title.replace('\n', " "), self.date.clone()];
        lines.extend(self.authors.iter().map(|a| a.replace('\n', " ")));
        lines.join("\n")
    }

    /// The date as (year, month, day), if it is a valid `YYYY-MM-DD`.
    pub fn ymd(&self) -> Option<(i32, u8, u8)> {
        let mut parts = self.date.trim().split('-');
        let y = parts.next()?.parse().ok()?;
        let m = parts
            .next()?
            .parse()
            .ok()
            .filter(|m| (1..=12).contains(m))?;
        let d = parts
            .next()?
            .parse()
            .ok()
            .filter(|d| (1..=31).contains(d))?;
        parts.next().is_none().then_some((y, m, d))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListKind {
    /// `- item`
    Bullet,
    /// `+ item`, numbered by Typst.
    Numbered,
    /// `/ Term: description`
    Term,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InlineNode {
    #[serde(skip)]
    pub id: NodeId,
    pub kind: Inline,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Inline {
    Text(String),
    Math(MathRow),
    /// Inline code, `` `…` ``: its text kept verbatim, set in a monospace
    /// font. Edited in place like text.
    Raw(String),
    /// A reference to a label, `@label`: an atom for the caret, rendered
    /// by Typst ("Section 2", "Equation (3)").
    Ref(String),
    /// A call with a content body in text: `#strong[…]` (written `*…*`),
    /// `#emph[…]` (`_…_`), `#cl[…]`, `#link("…")[…]`. Typst draws it; the
    /// body is edited in place and alternates like a paragraph's inlines.
    /// `args` are the other arguments as Typst source (`"https://…"`),
    /// empty if none.
    Call {
        name: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        args: String,
        body: Vec<InlineNode>,
    },
}

impl InlineNode {
    /// The body of an inline call.
    pub fn body(&self) -> Option<&Vec<InlineNode>> {
        match &self.kind {
            Inline::Call { body, .. } => Some(body),
            _ => None,
        }
    }

    pub fn body_mut(&mut self) -> Option<&mut Vec<InlineNode>> {
        match &mut self.kind {
            Inline::Call { body, .. } => Some(body),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match &self.kind {
            Inline::Text(_) => "text",
            Inline::Math(_) => "math",
            Inline::Raw(_) => "code",
            Inline::Ref(_) => "reference",
            Inline::Call { name, .. } => match name.as_str() {
                "strong" => "bold",
                "emph" => "italic",
                "link" => "link",
                _ => "function",
            },
        }
    }
}

/// The keys of the BibTeX entries embedded in Typst code (lines starting
/// with `@`, as in `#bibliography(bytes(```…```))`).
pub fn embedded_bib_keys(code: &str) -> Vec<String> {
    let entries: Vec<&str> = code
        .lines()
        .filter(|l| l.trim_start().starts_with('@'))
        .collect();
    bib_keys(&entries.join("\n"))
}

/// The entry keys of a bibliography source: BibTeX (`@article{key, …`; not
/// `@string`, `@comment`, `@preamble`) or Hayagriva YAML (top-level `key:`).
pub fn bib_keys(source: &str) -> Vec<String> {
    let bibtex = source.trim_start().starts_with('@') || source.trim_start().starts_with('%');
    let mut keys = vec![];
    for line in source.lines() {
        let key = if bibtex {
            let Some(rest) = line.trim_start().strip_prefix('@') else {
                continue;
            };
            let Some((kind, rest)) = rest.split_once('{') else {
                continue;
            };
            let kind = kind.trim().to_ascii_lowercase();
            if matches!(kind.as_str(), "string" | "comment" | "preamble") {
                continue;
            }
            rest.split(',').next().unwrap_or("").trim()
        } else {
            if line.starts_with([' ', '\t', '#', '-']) {
                continue;
            }
            match line.trim_end().strip_suffix(':') {
                Some(key) => key.trim(),
                None => continue,
            }
        };
        if !key.is_empty() && !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
    }
    keys
}

/// The formulas of an inline list, in calls too.
fn inline_rows<'a>(inlines: &'a [InlineNode], out: &mut Vec<&'a MathRow>) {
    for inline in inlines {
        match &inline.kind {
            Inline::Math(row) => out.push(row),
            Inline::Call { body, .. } => inline_rows(body, out),
            Inline::Text(_) | Inline::Raw(_) | Inline::Ref(_) => {}
        }
    }
}

fn inline_rows_mut<'a>(inlines: &'a mut [InlineNode], out: &mut Vec<&'a mut MathRow>) {
    for inline in inlines {
        match &mut inline.kind {
            Inline::Math(row) => out.push(row),
            Inline::Call { body, .. } => inline_rows_mut(body, out),
            Inline::Text(_) | Inline::Raw(_) | Inline::Ref(_) => {}
        }
    }
}

/// Whether inline `id` is in this list, in calls too.
fn inlines_contain(inlines: &[InlineNode], id: NodeId) -> bool {
    inlines
        .iter()
        .any(|i| i.id == id || i.body().is_some_and(|b| inlines_contain(b, id)))
}

/// The body of inline call `id` in this list, in calls too.
fn call_body_mut(inlines: &mut [InlineNode], id: NodeId) -> Option<&mut Vec<InlineNode>> {
    for inline in inlines {
        if inline.id == id {
            return inline.body_mut();
        }
        if let Some(body) = inline.body_mut()
            && let Some(found) = call_body_mut(body, id)
        {
            return Some(found);
        }
    }
    None
}

/// The value of the string literal that `args` (a call's arguments as
/// Typst source) start with: `"https://a.org", …` gives `https://a.org`.
/// Escapes other than `\"` and `\\` are not read.
fn string_literal(args: &str) -> Option<String> {
    let mut chars = args.trim_start().strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                c @ ('"' | '\\') => out.push(c),
                _ => return None,
            },
            c => out.push(c),
        }
    }
}

/// The inline `id` in this list, in calls too.
fn find_inline_mut(inlines: &mut [InlineNode], id: NodeId) -> Option<&mut InlineNode> {
    for inline in inlines {
        if inline.id == id {
            return Some(inline);
        }
        if let Some(body) = inline.body_mut()
            && let Some(found) = find_inline_mut(body, id)
        {
            return Some(found);
        }
    }
    None
}

/// A horizontal sequence of math nodes. An empty row renders as a slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MathRow {
    #[serde(skip)]
    pub id: NodeId,
    pub nodes: Vec<MathNode>,
}

/// An atom of a formula. Typst ignores spaces in math except next to a
/// bar or quoted text, where a written space shows (measured; Typst's
/// "spaced" items): `space` keeps one written before this node there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "MathNodeFile", into = "MathNodeFile")]
pub struct MathNode {
    pub id: NodeId,
    pub kind: MathKind,
    /// A space before this node, kept only where it shows (see
    /// [`spaced_gap`]).
    pub space: bool,
}

/// A node in a `.sem` file: just its kind, or `{"space": true, "kind": …}`
/// when a space comes before it (so files without spaces read as before).
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum MathNodeFile {
    Spaced { space: bool, kind: MathKind },
    Plain(MathKind),
}

impl From<MathNodeFile> for MathNode {
    fn from(f: MathNodeFile) -> Self {
        let (kind, space) = match f {
            MathNodeFile::Spaced { space, kind } => (kind, space),
            MathNodeFile::Plain(kind) => (kind, false),
        };
        MathNode {
            id: NodeId::default(),
            kind,
            space,
        }
    }
}

impl From<MathNode> for MathNodeFile {
    fn from(n: MathNode) -> Self {
        match n.space {
            true => MathNodeFile::Spaced {
                space: true,
                kind: n.kind,
            },
            false => MathNodeFile::Plain(n.kind),
        }
    }
}

/// Whether Typst shows a space written between `a` and `b`: next to a bar
/// or quoted text (Typst's spaced items), never after a line break.
pub fn spaced_gap(a: &MathNode, b: &MathNode) -> bool {
    a.kind != MathKind::Linebreak && (a.kind.is_spaced() || b.kind.is_spaced())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MathKind {
    Sym(String),
    Ident(String),
    Num(String),
    Frac {
        num: MathRow,
        den: MathRow,
    },
    Scripts {
        base: MathRow,
        sup: Option<MathRow>,
        sub: Option<MathRow>,
    },
    Slot,
    /// Typst code in a formula, written `#code`: an inline diagram
    /// (`box(cetz.canvas({…}))`), a call of a header function (`dia({…})`).
    /// An atom for the caret; its source is edited in the source dialog,
    /// like a diagram block's.
    Code(String),
    /// A call of a math function with content arguments, `sqrt(x)`,
    /// `cal(A)`, `root(3, x)`, `ket(psi)` (a header function): Typst draws
    /// it, the caret moves through the arguments. `opts` are its named
    /// arguments as Typst source, written before the content arguments
    /// (`size: #55%` in `lr(size: #55%, (x))`), empty if none.
    Call {
        name: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        opts: String,
        args: Vec<MathRow>,
    },
    /// Primes attached to what comes before, `f'`, `f''` (the count).
    Primes(usize),
    /// Text in a formula, `"if"`: upright text, spaces kept.
    Text(String),
    /// An alignment point, `&` (aligned multi-line equations).
    Align,
    /// A line break, `\`.
    Linebreak,
    /// An inline CeTZ drawing: like a diagram block, only the drawing code
    /// (the body of `cetz.canvas({ import cetz.draw: * … })`), set in a box
    /// in the formula.
    Diagram(String),
}

/// Typst's math functions that take options (named arguments that are not
/// content): their calls get an options tab. Checked against Typst's
/// library by a test in `typst-bridge`.
pub const OPTION_CALLS: &[&str] = &[
    "abs", "accent", "acute", "arrow", "bag", "bar", "brace", "bracket", "breve", "cancel",
    "caron", "cases", "ceil", "chevron", "circle", "compose", "corner", "dash", "diaer", "display",
    "dot", "fence", "floor", "frac", "grave", "harpoon", "hat", "inline", "limits", "lr", "macron",
    "mat", "minus", "mustache", "norm", "op", "paren", "round", "script", "shell", "sscript",
    "stretch", "tilde", "vec",
];

/// Bars Typst classes as fences: spaces written next to them show.
pub const BARS: [&str; 3] = ["|", "‖", "⦀"];

impl MathKind {
    /// The options of a call that has an options tab: one of Typst's
    /// functions that take options, or one with options already.
    pub fn tab(&self) -> Option<&str> {
        match self {
            MathKind::Call { name, opts, .. }
                if !opts.is_empty() || OPTION_CALLS.contains(&name.as_str()) =>
            {
                Some(opts)
            }
            _ => None,
        }
    }

    /// A bar or quoted text: Typst shows the spaces written next to it.
    pub fn is_spaced(&self) -> bool {
        match self {
            MathKind::Sym(s) => BARS.contains(&s.as_str()),
            MathKind::Text(_) => true,
            _ => false,
        }
    }
}

impl Block {
    /// Whether the block can have a label (headings, display formulas,
    /// environments).
    pub fn labelable(&self) -> bool {
        matches!(
            self,
            Block::Heading { .. } | Block::DisplayMath(_) | Block::Env { .. }
        )
    }

    /// The inline list of a paragraph or heading.
    pub fn inlines(&self) -> Option<&Vec<InlineNode>> {
        match self {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => Some(inlines),
            _ => None,
        }
    }

    pub fn inlines_mut(&mut self) -> Option<&mut Vec<InlineNode>> {
        match self {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => Some(inlines),
            _ => None,
        }
    }

    /// The inlines of a text block: a paragraph, heading or caption.
    pub fn own_inlines(&self) -> Option<&Vec<InlineNode>> {
        match self {
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines) => Some(inlines),
            _ => None,
        }
    }

    /// The inlines the caret can type in: a paragraph's, heading's or
    /// caption's, an environment's title or a term item's term.
    pub fn text_inlines(&self) -> Option<&Vec<InlineNode>> {
        match self {
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines) => Some(inlines),
            Block::Env { title, .. } | Block::Item { term: title, .. } => Some(title),
            _ => None,
        }
    }

    pub fn text_inlines_mut(&mut self) -> Option<&mut Vec<InlineNode>> {
        match self {
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines) => Some(inlines),
            Block::Env { title, .. } | Block::Item { term: title, .. } => Some(title),
            _ => None,
        }
    }

    /// The blocks of a container: an environment's or list item's body.
    pub fn body(&self) -> Option<&Vec<BlockNode>> {
        match self {
            Block::Env { body, .. } | Block::Item { body, .. } => Some(body),
            _ => None,
        }
    }

    pub fn body_mut(&mut self) -> Option<&mut Vec<BlockNode>> {
        match self {
            Block::Env { body, .. } | Block::Item { body, .. } => Some(body),
            _ => None,
        }
    }
}

impl BlockNode {
    /// The top-level math rows of this block.
    pub fn rows(&self) -> Vec<&MathRow> {
        match &self.kind {
            Block::DisplayMath(row) => vec![row],
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines)
            | Block::Env { title: inlines, .. }
            | Block::Item { term: inlines, .. } => {
                let mut out = vec![];
                inline_rows(inlines, &mut out);
                out
            }
            Block::RawTypst(_)
            | Block::Diagram(_)
            | Block::Bibliography(_)
            | Block::Title(_)
            | Block::Image { .. } => vec![],
        }
    }

    pub fn rows_mut(&mut self) -> Vec<&mut MathRow> {
        match &mut self.kind {
            Block::DisplayMath(row) => vec![row],
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines)
            | Block::Env { title: inlines, .. }
            | Block::Item { term: inlines, .. } => {
                let mut out = vec![];
                inline_rows_mut(inlines, &mut out);
                out
            }
            Block::RawTypst(_)
            | Block::Diagram(_)
            | Block::Bibliography(_)
            | Block::Title(_)
            | Block::Image { .. } => vec![],
        }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            Block::Heading { .. } => "heading",
            Block::Paragraph(_) => "paragraph",
            Block::DisplayMath(_) => "display math",
            Block::RawTypst(_) => "raw typst",
            Block::Diagram(_) => "diagram",
            Block::Image { .. } => "picture",
            Block::Env { .. } => "environment",
            Block::Item { .. } => "list item",
            Block::Caption(_) => "caption",
            Block::Bibliography(_) => "bibliography",
            Block::Title(_) => "title",
        }
    }

    /// The references in this block (nested blocks and inline calls too).
    pub fn ref_ids(&self) -> Vec<NodeId> {
        fn inlines(v: &[InlineNode], out: &mut Vec<NodeId>) {
            for inline in v {
                match &inline.kind {
                    Inline::Ref(_) => out.push(inline.id),
                    Inline::Call { body, .. } => inlines(body, out),
                    _ => {}
                }
            }
        }
        let mut out = vec![];
        let mut all = vec![];
        all_blocks(std::slice::from_ref(self), &mut all);
        for block in all {
            if let Some(v) = block.kind.text_inlines() {
                inlines(v, &mut out);
            }
        }
        out
    }

    /// The hyperlinks (`#link(…)[…]` calls) in this block and its nested
    /// blocks: the block holding each and the path prefix of the caret
    /// positions inside its text (ending with `Body`).
    pub fn links(&self) -> Vec<(NodeId, Vec<Step>)> {
        fn inlines(
            block: NodeId,
            v: &[InlineNode],
            prefix: &[Step],
            out: &mut Vec<(NodeId, Vec<Step>)>,
        ) {
            for (i, inline) in v.iter().enumerate() {
                if let Inline::Call { name, body, .. } = &inline.kind {
                    let mut path = prefix.to_vec();
                    path.extend([Step::Index(i), Step::Body]);
                    if name == "link" {
                        out.push((block, path.clone()));
                    }
                    inlines(block, body, &path, out);
                }
            }
        }
        let mut out = vec![];
        let mut all = vec![];
        all_blocks(std::slice::from_ref(self), &mut all);
        for block in all {
            if let Some(v) = block.kind.text_inlines() {
                inlines(block.id, v, &[], &mut out);
            }
        }
        out
    }

    /// Whether `id` is this block or inside it.
    pub fn contains(&self, id: NodeId) -> bool {
        self.id == id
            || self
                .kind
                .body()
                .is_some_and(|body| body.iter().any(|b| b.contains(id)))
    }
}

/// Where a block sits: at `index` in the document's block list (`parent`
/// `None`) or in the body of environment `parent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub parent: Option<NodeId>,
    pub index: usize,
}

/// Blocks in document order, environments before their bodies.
fn all_blocks<'a>(blocks: &'a [BlockNode], out: &mut Vec<&'a BlockNode>) {
    for block in blocks {
        out.push(block);
        if let Some(body) = block.kind.body() {
            all_blocks(body, out);
        }
    }
}

fn leaves_mut<'a>(blocks: &'a mut [BlockNode], out: &mut Vec<&'a mut BlockNode>) {
    for block in blocks {
        if block.kind.body().is_some() {
            leaves_mut(block.kind.body_mut().expect("container"), out);
        } else {
            out.push(block);
        }
    }
}

/// The math row with this id, in any block at any depth.
fn find_row_in(blocks: &mut [BlockNode], id: NodeId) -> Option<&mut MathRow> {
    for block in blocks {
        if block.rows().iter().any(|r| r.find_row(id).is_some()) {
            return block
                .rows_mut()
                .into_iter()
                .find_map(|r| r.find_row_mut(id));
        }
        if let Some(body) = block.kind.body_mut()
            && let Some(row) = find_row_in(body, id)
        {
            return Some(row);
        }
    }
    None
}

fn place_in(blocks: &[BlockNode], parent: Option<NodeId>, id: NodeId) -> Option<Place> {
    blocks.iter().enumerate().find_map(|(index, b)| {
        if b.id == id {
            Some(Place { parent, index })
        } else {
            place_in(b.kind.body()?, Some(b.id), id)
        }
    })
}

impl MathNode {
    /// The child row named by `step`, if this node has one.
    pub fn child(&self, step: Step) -> Option<&MathRow> {
        match (&self.kind, step) {
            (MathKind::Frac { num, .. }, Step::Num) => Some(num),
            (MathKind::Frac { den, .. }, Step::Den) => Some(den),
            (MathKind::Scripts { base, .. }, Step::Base) => Some(base),
            (MathKind::Scripts { sup, .. }, Step::Sup) => sup.as_ref(),
            (MathKind::Scripts { sub, .. }, Step::Sub) => sub.as_ref(),
            (MathKind::Call { args, .. }, Step::Arg(i)) => args.get(i),
            _ => None,
        }
    }

    pub fn child_mut(&mut self, step: Step) -> Option<&mut MathRow> {
        match (&mut self.kind, step) {
            (MathKind::Frac { num, .. }, Step::Num) => Some(num),
            (MathKind::Frac { den, .. }, Step::Den) => Some(den),
            (MathKind::Scripts { base, .. }, Step::Base) => Some(base),
            (MathKind::Scripts { sup, .. }, Step::Sup) => sup.as_mut(),
            (MathKind::Scripts { sub, .. }, Step::Sub) => sub.as_mut(),
            (MathKind::Call { args, .. }, Step::Arg(i)) => args.get_mut(i),
            _ => None,
        }
    }

    /// Child rows in visual/navigation order.
    pub fn children(&self) -> Vec<(Step, &MathRow)> {
        if let MathKind::Call { args, .. } = &self.kind {
            return args
                .iter()
                .enumerate()
                .map(|(i, r)| (Step::Arg(i), r))
                .collect();
        }
        [Step::Num, Step::Den, Step::Base, Step::Sup, Step::Sub]
            .into_iter()
            .filter_map(|s| self.child(s).map(|r| (s, r)))
            .collect()
    }

    pub fn children_mut(&mut self) -> Vec<&mut MathRow> {
        match &mut self.kind {
            MathKind::Frac { num, den } => vec![num, den],
            MathKind::Scripts { base, sup, sub } => {
                let mut v = vec![base];
                v.extend(sup.as_mut());
                v.extend(sub.as_mut());
                v
            }
            MathKind::Call { args, .. } => args.iter_mut().collect(),
            _ => vec![],
        }
    }

    /// The source of a code atom or inline diagram, edited in the source
    /// dialog.
    pub fn source(&self) -> Option<&str> {
        match &self.kind {
            MathKind::Code(s) | MathKind::Diagram(s) => Some(s),
            _ => None,
        }
    }

    /// True for nodes with child rows (fractions, scripts, calls).
    pub fn is_structure(&self) -> bool {
        matches!(
            self.kind,
            MathKind::Frac { .. } | MathKind::Scripts { .. } | MathKind::Call { .. }
        )
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            MathKind::Sym(_) => "symbol",
            MathKind::Ident(_) => "identifier",
            MathKind::Num(_) => "number",
            MathKind::Frac { .. } => "fraction",
            MathKind::Scripts { .. } => "scripts",
            MathKind::Slot => "slot",
            MathKind::Code(_) => "code",
            MathKind::Diagram(_) => "diagram",
            MathKind::Call { .. } => "function",
            MathKind::Primes(_) => "primes",
            MathKind::Text(_) => "text",
            MathKind::Align => "alignment point",
            MathKind::Linebreak => "line break",
        }
    }
}

impl MathRow {
    /// The row holding node `id` (at any depth) and the node's index in it.
    pub fn find_node(&self, id: NodeId) -> Option<(&MathRow, usize)> {
        if let Some(i) = self.nodes.iter().position(|n| n.id == id) {
            return Some((self, i));
        }
        self.nodes
            .iter()
            .flat_map(|n| n.children().into_iter().map(|(_, r)| r))
            .find_map(|r| r.find_node(id))
    }

    pub fn find_row(&self, id: NodeId) -> Option<&MathRow> {
        if self.id == id {
            return Some(self);
        }
        self.nodes
            .iter()
            .flat_map(|n| n.children().into_iter().map(|(_, r)| r))
            .find_map(|r| r.find_row(id))
    }

    pub fn find_row_mut(&mut self, id: NodeId) -> Option<&mut MathRow> {
        if self.id == id {
            return Some(self);
        }
        self.nodes
            .iter_mut()
            .flat_map(|n| n.children_mut())
            .find_map(|r| r.find_row_mut(id))
    }

    fn visit_ids(&mut self, f: &mut impl FnMut(&mut NodeId)) {
        f(&mut self.id);
        for node in &mut self.nodes {
            f(&mut node.id);
            for row in node.children_mut() {
                row.visit_ids(f);
            }
        }
    }
}

fn visit_inline_ids(inlines: &mut [InlineNode], f: &mut impl FnMut(&mut NodeId)) {
    for inline in inlines {
        f(&mut inline.id);
        match &mut inline.kind {
            Inline::Math(row) => row.visit_ids(f),
            Inline::Call { body, .. } => visit_inline_ids(body, f),
            Inline::Text(_) | Inline::Raw(_) | Inline::Ref(_) => {}
        }
    }
}

/// Visit the ids of `blocks` and of everything in them.
fn visit_block_ids(blocks: &mut [BlockNode], f: &mut impl FnMut(&mut NodeId)) {
    for block in blocks {
        f(&mut block.id);
        match &mut block.kind {
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines) => {
                visit_inline_ids(inlines, f);
            }
            Block::DisplayMath(row) => row.visit_ids(f),
            Block::Env { title, body, .. }
            | Block::Item {
                term: title, body, ..
            } => {
                visit_inline_ids(title, f);
                visit_block_ids(body, f);
            }
            Block::RawTypst(_)
            | Block::Diagram(_)
            | Block::Bibliography(_)
            | Block::Title(_)
            | Block::Image { .. } => {}
        }
    }
}

/// The whole document: a list of blocks; environments hold blocks of their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Typst code compiled before every block and at the top of the export:
    /// set/show rules, imports, definitions. Should not produce content.
    pub header: String,
    pub blocks: Vec<BlockNode>,
    /// The project's other files (pictures, bibliography files), stored
    /// beside the document in the project file.
    #[serde(skip)]
    pub files: crate::Files,
    /// Remarks on blocks and ranges, stored beside the document.
    #[serde(skip)]
    pub remarks: Vec<crate::Remark>,
    #[serde(skip)]
    next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// A document with one empty paragraph.
    pub fn new() -> Self {
        let mut doc = Self::empty();
        let p = doc.paragraph(vec![]);
        doc.blocks.push(p);
        doc
    }

    /// A document with exactly the given blocks (built with this document's
    /// builders, or regenerated afterwards).
    pub fn empty() -> Self {
        Self {
            header: String::new(),
            blocks: vec![],
            files: crate::Files::default(),
            remarks: vec![],
            next_id: 1,
        }
    }

    /// Never hand out `id` (a deleted block a detached remark points at).
    pub fn reserve_id(&mut self, id: NodeId) {
        self.next_id = self.next_id.max(id.0 + 1);
    }

    pub fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Assign new ids to every node, e.g. after deserialization, and restore
    /// the structural invariants.
    pub fn regenerate_ids(&mut self) {
        let mut next = 1;
        visit_block_ids(&mut self.blocks, &mut |id: &mut NodeId| {
            *id = NodeId(next);
            next += 1;
        });
        self.next_id = next;
        self.normalize();
    }

    /// After loading: blocks keep their saved ids (a repeated one only the
    /// first time); every other node, and a block without one, gets a fresh
    /// id above them. Restores the structural invariants.
    pub fn restore_ids(&mut self) {
        fn keep(blocks: &mut [BlockNode], seen: &mut std::collections::HashSet<NodeId>) {
            for block in blocks {
                if !block.id.is_unset() && !seen.insert(block.id) {
                    block.id = NodeId(0);
                }
                if let Block::Env { body, .. } | Block::Item { body, .. } = &mut block.kind {
                    keep(body, seen);
                }
            }
        }
        fn assign(blocks: &mut [BlockNode], fresh: &mut impl FnMut(&mut NodeId)) {
            for block in blocks {
                if block.id.is_unset() {
                    fresh(&mut block.id);
                }
                match &mut block.kind {
                    Block::Paragraph(inlines)
                    | Block::Heading { inlines, .. }
                    | Block::Caption(inlines) => visit_inline_ids(inlines, fresh),
                    Block::DisplayMath(row) => row.visit_ids(fresh),
                    Block::Env { title, body, .. }
                    | Block::Item {
                        term: title, body, ..
                    } => {
                        visit_inline_ids(title, fresh);
                        assign(body, fresh);
                    }
                    Block::RawTypst(_)
                    | Block::Diagram(_)
                    | Block::Bibliography(_)
                    | Block::Title(_)
                    | Block::Image { .. } => {}
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        keep(&mut self.blocks, &mut seen);
        let mut next = seen.iter().map(|id| id.0).max().unwrap_or(0) + 1;
        assign(&mut self.blocks, &mut |id: &mut NodeId| {
            *id = NodeId(next);
            next += 1;
        });
        self.next_id = next;
        self.normalize();
    }

    /// Give every node of `fragment` (from another document: a paste) a
    /// fresh id of this document.
    pub fn adopt(&mut self, fragment: &mut crate::Fragment) {
        let mut next = self.next_id;
        let mut f = |id: &mut NodeId| {
            *id = NodeId(next);
            next += 1;
        };
        match fragment {
            crate::Fragment::Blocks(blocks) => visit_block_ids(blocks, &mut f),
            crate::Fragment::Inlines(inlines) => visit_inline_ids(inlines, &mut f),
            crate::Fragment::Math(nodes) => {
                for node in nodes {
                    f(&mut node.id);
                    for row in node.children_mut() {
                        row.visit_ids(&mut f);
                    }
                }
            }
            crate::Fragment::Plain(_) => {}
        }
        self.next_id = next;
    }

    /// Enforce the paragraph invariant: inlines alternate Text, Math, Text, …,
    /// starting and ending with (possibly empty) Text. Keeps a caret position
    /// before and after every inline formula. A document and every
    /// environment body always have a block.
    pub fn normalize(&mut self) {
        let mut blocks = std::mem::take(&mut self.blocks);
        self.normalize_blocks(&mut blocks);
        self.blocks = blocks;
    }

    fn normalize_blocks(&mut self, blocks: &mut Vec<BlockNode>) {
        for block in blocks.iter_mut() {
            if let Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Caption(inlines) = &mut block.kind
            {
                let old = std::mem::take(inlines);
                *inlines = self.normalized_inlines(old);
            } else if let Block::Env { title, body, .. }
            | Block::Item {
                term: title, body, ..
            } = &mut block.kind
            {
                if !title.is_empty() {
                    let old = std::mem::take(title);
                    *title = self.normalized_inlines(old);
                }
                self.normalize_blocks(body);
            }
        }
        if blocks.is_empty() {
            blocks.push(self.paragraph(vec![]));
        }
    }

    /// Inlines alternating Text, Math or Call, Text, …, starting and ending
    /// with text; call bodies too.
    pub fn normalized_inlines(&mut self, inlines: Vec<InlineNode>) -> Vec<InlineNode> {
        let mut out: Vec<InlineNode> = Vec::with_capacity(inlines.len() + 2);
        for mut inline in inlines {
            if let Some(body) = inline.body_mut() {
                let old = std::mem::take(body);
                let new = self.normalized_inlines(old);
                *inline.body_mut().expect("a call") = new;
            }
            match (&inline.kind, out.last_mut()) {
                (
                    Inline::Text(s),
                    Some(InlineNode {
                        kind: Inline::Text(prev),
                        ..
                    }),
                ) => {
                    prev.push_str(s);
                }
                (Inline::Math(_) | Inline::Call { .. } | Inline::Raw(_) | Inline::Ref(_), last)
                    if last
                        .as_ref()
                        .is_none_or(|l| !matches!(l.kind, Inline::Text(_))) =>
                {
                    out.push(self.text(""));
                    out.push(inline);
                }
                _ => out.push(inline),
            }
        }
        if !matches!(
            out.last(),
            Some(InlineNode {
                kind: Inline::Text(_),
                ..
            })
        ) {
            out.push(self.text(""));
        }
        out
    }

    /// Check structural invariants (used by tests and debug assertions).
    pub fn check_invariants(&self) -> Result<(), String> {
        if self.blocks.is_empty() {
            return Err("document has no blocks".into());
        }
        let mut seen = std::collections::HashSet::new();
        let mut check = |id: NodeId| -> Result<(), String> {
            if id.0 == 0 || id.0 >= self.next_id || !seen.insert(id) {
                Err(format!("bad or duplicate id {id:?}"))
            } else {
                Ok(())
            }
        };
        fn row_ids(
            row: &MathRow,
            check: &mut impl FnMut(NodeId) -> Result<(), String>,
        ) -> Result<(), String> {
            check(row.id)?;
            for node in &row.nodes {
                check(node.id)?;
                for (_, child) in node.children() {
                    row_ids(child, check)?;
                }
            }
            Ok(())
        }
        fn inline_ids(
            owner: NodeId,
            inlines: &[InlineNode],
            check: &mut impl FnMut(NodeId) -> Result<(), String>,
        ) -> Result<(), String> {
            for (i, inline) in inlines.iter().enumerate() {
                check(inline.id)?;
                let want_text = i % 2 == 0;
                match &inline.kind {
                    Inline::Text(_) if want_text => {}
                    Inline::Math(row) if !want_text => row_ids(row, check)?,
                    Inline::Call { body, .. } if !want_text => inline_ids(inline.id, body, check)?,
                    Inline::Raw(_) | Inline::Ref(_) if !want_text => {}
                    _ => return Err(format!("{owner:?}: inline {i} breaks alternation")),
                }
            }
            if inlines.len().is_multiple_of(2) {
                return Err(format!("{owner:?}: must start and end with text"));
            }
            Ok(())
        }
        let mut all = vec![];
        all_blocks(&self.blocks, &mut all);
        for block in all {
            check(block.id)?;
            if block.kind.body().is_some_and(|b| b.is_empty()) {
                return Err(format!("environment {:?} has an empty body", block.id));
            }
            if let Some(inlines) = block.kind.text_inlines()
                && !(inlines.is_empty() && block.kind.body().is_some())
            {
                inline_ids(block.id, inlines, &mut check)?;
            } else if let Block::DisplayMath(row) = &block.kind {
                row_ids(row, &mut check)?;
            }
        }
        Ok(())
    }

    /// The block with this id, at any depth.
    pub fn block(&self, id: NodeId) -> Option<&BlockNode> {
        let place = self.place(id)?;
        self.siblings(place.parent)?.get(place.index)
    }

    pub fn block_mut(&mut self, id: NodeId) -> Option<&mut BlockNode> {
        let place = self.place(id)?;
        self.siblings_mut(place.parent)?.get_mut(place.index)
    }

    /// Where the block with this id sits.
    pub fn place(&self, id: NodeId) -> Option<Place> {
        place_in(&self.blocks, None, id)
    }

    /// The document's blocks (`None`) or an environment's body.
    pub fn siblings(&self, parent: Option<NodeId>) -> Option<&Vec<BlockNode>> {
        match parent {
            None => Some(&self.blocks),
            Some(env) => self.block(env)?.kind.body(),
        }
    }

    fn siblings_mut(&mut self, parent: Option<NodeId>) -> Option<&mut Vec<BlockNode>> {
        match parent {
            None => Some(&mut self.blocks),
            Some(env) => self.block_mut(env)?.kind.body_mut(),
        }
    }

    /// The block at `place`.
    pub fn at(&self, place: Place) -> Option<&BlockNode> {
        self.siblings(place.parent)?.get(place.index)
    }

    /// The block next to the one at `place` (`delta` -1 or +1), in the same
    /// list.
    pub fn sibling(&self, place: Place, delta: isize) -> Option<&BlockNode> {
        let index = place.index.checked_add_signed(delta)?;
        self.siblings(place.parent)?.get(index)
    }

    /// The top-level block that is or contains node `id`: a block, or an
    /// inline, row or math node in it.
    pub fn top_block(&self, id: NodeId) -> Option<NodeId> {
        let holds = |b: &BlockNode| {
            let mut all = vec![];
            all_blocks(std::slice::from_ref(b), &mut all);
            all.into_iter().any(|b| {
                b.id == id
                    || b.kind
                        .text_inlines()
                        .is_some_and(|v| inlines_contain(v, id))
                    || b.rows()
                        .into_iter()
                        .any(|r| r.find_row(id).is_some() || r.find_node(id).is_some())
            })
        };
        self.blocks.iter().find(|b| holds(b)).map(|b| b.id)
    }

    /// Index of the top-level block that is or contains `id`.
    pub fn top_index(&self, id: NodeId) -> Option<usize> {
        self.blocks.iter().position(|b| b.contains(id))
    }

    /// The blocks the caret can be in (all but containers), in document
    /// order.
    pub fn leaves(&self) -> Vec<&BlockNode> {
        let mut all = vec![];
        all_blocks(&self.blocks, &mut all);
        all.retain(|b| b.kind.body().is_none());
        all
    }

    pub fn leaves_mut(&mut self) -> Vec<&mut BlockNode> {
        let mut out = vec![];
        leaves_mut(&mut self.blocks, &mut out);
        out
    }

    pub fn find_row(&self, id: NodeId) -> Option<&MathRow> {
        let mut all = vec![];
        all_blocks(&self.blocks, &mut all);
        all.into_iter()
            .flat_map(|b| b.rows())
            .find_map(|r| r.find_row(id))
    }

    pub fn find_row_mut(&mut self, id: NodeId) -> Option<&mut MathRow> {
        find_row_in(&mut self.blocks, id)
    }

    /// The row holding math node `id`, at any depth, and the node's index.
    pub fn find_node(&self, id: NodeId) -> Option<(&MathRow, usize)> {
        let mut all = vec![];
        all_blocks(&self.blocks, &mut all);
        all.into_iter()
            .flat_map(|b| b.rows())
            .find_map(|r| r.find_node(id))
    }

    /// The inline list owned by `owner`: a block's text inlines or an inline
    /// call's body.
    pub fn inlines_of_mut(&mut self, owner: NodeId) -> Option<&mut Vec<InlineNode>> {
        if self.block(owner).is_some() {
            return self.block_mut(owner)?.kind.text_inlines_mut();
        }
        let block = self.inline_block(owner)?;
        let inlines = self.block_mut(block)?.kind.text_inlines_mut()?;
        call_body_mut(inlines, owner)
    }

    /// The inline `id` (text, formula or call), at any depth.
    pub fn inline_mut(&mut self, id: NodeId) -> Option<&mut InlineNode> {
        let block = self.inline_block(id)?;
        let inlines = self.block_mut(block)?.kind.text_inlines_mut()?;
        find_inline_mut(inlines, id)
    }

    /// The block whose inlines hold inline `id` (at any depth).
    fn inline_block(&self, id: NodeId) -> Option<NodeId> {
        let mut all = vec![];
        all_blocks(&self.blocks, &mut all);
        all.into_iter()
            .find(|b| {
                b.kind
                    .text_inlines()
                    .is_some_and(|v| inlines_contain(v, id))
            })
            .map(|b| b.id)
    }

    /// The bibliography block (the first, if there are several): the
    /// editor's, or raw Typst calling `#bibliography(…)` (written by hand,
    /// with a title or style).
    pub fn bibliography(&self) -> Option<&BlockNode> {
        self.blocks.iter().find(|b| match &b.kind {
            Block::Bibliography(_) => true,
            Block::RawTypst(s) => s.trim_start().starts_with("#bibliography("),
            _ => false,
        })
    }

    /// The citation keys of the bibliography, in its order (of the BibTeX
    /// entries embedded in a raw one).
    pub fn bib_keys(&self) -> Vec<String> {
        match self.bibliography().map(|b| &b.kind) {
            Some(Block::Bibliography(source)) => bib_keys(source),
            Some(Block::RawTypst(source)) => embedded_bib_keys(source),
            _ => vec![],
        }
    }

    /// The label of reference `id`, if `id` is a reference.
    pub fn ref_label(&self, id: NodeId) -> Option<&str> {
        fn find(inlines: &[InlineNode], id: NodeId) -> Option<&str> {
            inlines.iter().find_map(|i| match &i.kind {
                Inline::Ref(label) if i.id == id => Some(label.as_str()),
                Inline::Call { body, .. } => find(body, id),
                _ => None,
            })
        }
        let block = self.block(self.inline_block(id)?)?;
        find(block.kind.text_inlines()?, id)
    }

    /// The web address of the link that is or holds inline `id`,
    /// `#link("https://…")[…]`: `http`, `https` and `mailto` only (not a
    /// link to a label or a local file).
    pub fn link_url(&self, id: NodeId) -> Option<String> {
        /// The arguments of the innermost link around `id` (`link` around
        /// this list), if `id` is in it.
        fn find<'a>(
            v: &'a [InlineNode],
            id: NodeId,
            link: Option<&'a str>,
        ) -> Option<Option<&'a str>> {
            for inline in v {
                let here = match &inline.kind {
                    Inline::Call { name, args, .. } if name == "link" => Some(args.as_str()),
                    _ => link,
                };
                if inline.id == id {
                    return Some(here);
                }
                if let Some(body) = inline.body()
                    && let Some(found) = find(body, id, here)
                {
                    return Some(found);
                }
            }
            None
        }
        let block = self.block(self.inline_block(id)?)?;
        let args = find(block.kind.text_inlines()?, id, None)??;
        let url = string_literal(args)?;
        ["http://", "https://", "mailto:"]
            .iter()
            .any(|scheme| url.starts_with(scheme))
            .then_some(url)
    }

    /// Every label in document order, with the block it labels.
    pub fn labels(&self) -> Vec<(&str, &BlockNode)> {
        let mut all = vec![];
        all_blocks(&self.blocks, &mut all);
        all.into_iter()
            .filter(|b| !b.label.is_empty() && b.kind.labelable())
            .map(|b| (b.label.as_str(), b))
            .collect()
    }

    /// The source of code atom or inline diagram `id`.
    pub fn code_source(&self, id: NodeId) -> Option<&str> {
        let (row, i) = self.find_node(id)?;
        row.nodes[i].source()
    }

    /// Whether `id` is a list item.
    pub fn is_item(&self, id: NodeId) -> bool {
        matches!(self.block(id).map(|b| &b.kind), Some(Block::Item { .. }))
    }

    /// Whether `id` is an environment.
    pub fn is_env(&self, id: NodeId) -> bool {
        matches!(self.block(id).map(|b| &b.kind), Some(Block::Env { .. }))
    }

    /// The innermost environment that is or contains block `id`.
    pub fn env_around(&self, id: NodeId) -> Option<NodeId> {
        let mut id = id;
        loop {
            if self.is_env(id) {
                return Some(id);
            }
            id = self.place(id)?.parent?;
        }
    }

    // ----- Builders. They allocate fresh ids from this document. -----

    pub fn text(&mut self, s: &str) -> InlineNode {
        InlineNode {
            id: self.fresh_id(),
            kind: Inline::Text(s.into()),
        }
    }

    /// A reference to `label`, `@label`.
    pub fn reference(&mut self, label: &str) -> InlineNode {
        InlineNode {
            id: self.fresh_id(),
            kind: Inline::Ref(label.into()),
        }
    }

    pub fn raw_inline(&mut self, s: &str) -> InlineNode {
        InlineNode {
            id: self.fresh_id(),
            kind: Inline::Raw(s.into()),
        }
    }

    pub fn math(&mut self, nodes: Vec<MathNode>) -> InlineNode {
        let row = self.row(nodes);
        InlineNode {
            id: self.fresh_id(),
            kind: Inline::Math(row),
        }
    }

    /// An inline call `#name(args)[body]`; the body is normalized.
    pub fn inline_call(&mut self, name: &str, args: &str, body: Vec<InlineNode>) -> InlineNode {
        let body = self.normalized_inlines(body);
        InlineNode {
            id: self.fresh_id(),
            kind: Inline::Call {
                name: name.into(),
                args: args.into(),
                body,
            },
        }
    }

    pub fn row(&mut self, nodes: Vec<MathNode>) -> MathRow {
        MathRow {
            id: self.fresh_id(),
            nodes,
        }
    }

    pub fn node(&mut self, kind: MathKind) -> MathNode {
        MathNode {
            id: self.fresh_id(),
            kind,
            space: false,
        }
    }

    pub fn ident(&mut self, s: &str) -> MathNode {
        self.node(MathKind::Ident(s.into()))
    }

    pub fn num(&mut self, s: &str) -> MathNode {
        self.node(MathKind::Num(s.into()))
    }

    pub fn sym(&mut self, s: &str) -> MathNode {
        self.node(MathKind::Sym(s.into()))
    }

    pub fn slot(&mut self) -> MathNode {
        self.node(MathKind::Slot)
    }

    pub fn code(&mut self, source: &str) -> MathNode {
        self.node(MathKind::Code(source.into()))
    }

    pub fn call(&mut self, name: &str, args: Vec<MathRow>) -> MathNode {
        self.call_with(name, "", args)
    }

    /// A math call with named arguments (`opts`, Typst source).
    pub fn call_with(&mut self, name: &str, opts: &str, args: Vec<MathRow>) -> MathNode {
        self.node(MathKind::Call {
            name: name.into(),
            opts: opts.into(),
            args,
        })
    }

    pub fn inline_diagram(&mut self, source: &str) -> MathNode {
        self.node(MathKind::Diagram(source.into()))
    }

    pub fn frac(&mut self, num: Vec<MathNode>, den: Vec<MathNode>) -> MathNode {
        let num = self.row(num);
        let den = self.row(den);
        self.node(MathKind::Frac { num, den })
    }

    pub fn scripts(
        &mut self,
        base: Vec<MathNode>,
        sup: Option<Vec<MathNode>>,
        sub: Option<Vec<MathNode>>,
    ) -> MathNode {
        let base = self.row(base);
        let sup = sup.map(|n| self.row(n));
        let sub = sub.map(|n| self.row(n));
        self.node(MathKind::Scripts { base, sup, sub })
    }

    /// A paragraph; the inlines are normalized to the alternation invariant.
    pub fn paragraph(&mut self, inlines: Vec<InlineNode>) -> BlockNode {
        let inlines = self.normalized_inlines(inlines);
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Paragraph(inlines),
        }
    }

    /// A figure's caption (the last block of its body).
    pub fn caption(&mut self, inlines: Vec<InlineNode>) -> BlockNode {
        let inlines = self.normalized_inlines(inlines);
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Caption(inlines),
        }
    }

    pub fn heading(&mut self, level: u8, inlines: Vec<InlineNode>) -> BlockNode {
        let inlines = self.normalized_inlines(inlines);
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Heading { level, inlines },
        }
    }

    pub fn display(&mut self, nodes: Vec<MathNode>) -> BlockNode {
        let row = self.row(nodes);
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::DisplayMath(row),
        }
    }

    pub fn raw(&mut self, s: &str) -> BlockNode {
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::RawTypst(s.into()),
        }
    }

    pub fn bibliography_block(&mut self, source: &str) -> BlockNode {
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Bibliography(source.into()),
        }
    }

    pub fn title_block(&mut self, source: &str) -> BlockNode {
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Title(source.into()),
        }
    }

    /// The title block, if there is one.
    pub fn title(&self) -> Option<TitleFields> {
        self.blocks.iter().find_map(|b| match &b.kind {
            Block::Title(source) => Some(TitleFields::parse(source)),
            _ => None,
        })
    }

    pub fn diagram(&mut self, source: &str) -> BlockNode {
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Diagram(source.into()),
        }
    }

    /// A picture of project file `path`, with no options.
    pub fn image(&mut self, path: &str) -> BlockNode {
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Image {
                path: path.into(),
                opts: String::new(),
            },
        }
    }

    /// A list item; an empty body gets an empty paragraph, a term item an
    /// empty term.
    pub fn item(&mut self, kind: ListKind, mut body: Vec<BlockNode>) -> BlockNode {
        if body.is_empty() {
            body.push(self.paragraph(vec![]));
        }
        let term = match kind {
            ListKind::Term => vec![self.text("")],
            _ => vec![],
        };
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Item { kind, term, body },
        }
    }

    /// An environment (`#name[…]`); an empty body gets an empty paragraph.
    pub fn env(&mut self, name: &str, mut body: Vec<BlockNode>) -> BlockNode {
        if body.is_empty() {
            body.push(self.paragraph(vec![]));
        }
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Env {
                name: name.into(),
                args: String::new(),
                title: vec![],
                body,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraph_builder_normalizes() {
        let mut doc = Document::empty();
        let x = doc.ident("x");
        let m1 = doc.math(vec![x]);
        let m2 = doc.math(vec![]);
        let a = doc.text("a");
        let b = doc.text("b");
        let p = doc.paragraph(vec![m1, a, b, m2]);
        doc.blocks.push(p);
        doc.check_invariants().unwrap();
        let inl = doc.blocks[0].kind.inlines().unwrap();
        assert_eq!(inl.len(), 5);
        assert_eq!(inl[2].kind, Inline::Text("ab".into()));
    }

    #[test]
    fn bibliography_keys() {
        let bibtex = "@string{j = {J. Tests}}
@article{alpha,
  title = {A},
}
  @Book{ beta ,
  title = {B}}
@comment{not:this}";
        assert_eq!(bib_keys(bibtex), ["alpha", "beta"]);
        let yaml = "# works
alpha:
  type: article
  title: A
beta-2:
  type: book
";
        assert_eq!(bib_keys(yaml), ["alpha", "beta-2"]);
    }

    #[test]
    fn json_roundtrip_keeps_block_ids() {
        let mut doc = Document::empty();
        let x = doc.ident("x");
        let two = doc.num("2");
        let s = doc.scripts(vec![x], Some(vec![two]), None);
        let p = doc.math(vec![s]);
        let t = doc.text("see ");
        let para = doc.paragraph(vec![t, p]);
        let a = doc.ident("a");
        let b = doc.ident("b");
        let f = doc.frac(vec![a], vec![b]);
        let d = doc.display(vec![f]);
        doc.blocks = vec![para, d];

        doc.blocks[1].label = "eq:frac".into();
        let ids = |d: &Document| d.blocks.iter().map(|b| b.id).collect::<Vec<_>>();
        let json = serde_json::to_string(&doc).unwrap();
        // Only the blocks' ids are written; blocks without a label are
        // written as before labels existed.
        assert_eq!(json.matches("\"id\"").count(), 2, "{json}");
        assert!(
            json.contains("\"Paragraph\":[{\"Text\":\"see \"}"),
            "{json}"
        );
        assert!(json.contains("\"label\":\"eq:frac\""), "{json}");
        let mut back: Document = serde_json::from_str(&json).unwrap();
        back.restore_ids();
        back.check_invariants().unwrap();
        assert_eq!(ids(&back), ids(&doc));
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        // New nodes get ids above the saved ones.
        let fresh = back.fresh_id();
        assert!(ids(&back).iter().all(|id| *id < fresh));

        // A file from before ids were saved: fresh ids.
        let mut old: Document =
            serde_json::from_str(&json.replace(&format!("\"id\":{},", doc.blocks[0].id.0), ""))
                .unwrap();
        old.restore_ids();
        old.check_invariants().unwrap();
        assert_eq!(old.blocks[1].id, doc.blocks[1].id);
        assert!(!old.blocks[0].id.is_unset());
        // A repeated id (a hand-edited file) is kept only once.
        let mut twice = back.clone();
        twice.blocks[1].id = twice.blocks[0].id;
        let mut twice: Document =
            serde_json::from_str(&serde_json::to_string(&twice).unwrap()).unwrap();
        twice.restore_ids();
        twice.check_invariants().unwrap();
        assert_eq!(twice.blocks[0].id, doc.blocks[0].id);
        assert_ne!(twice.blocks[1].id, doc.blocks[0].id);
    }

    /// Blocks inside environments keep their ids too.
    #[test]
    fn nested_block_ids_are_kept() {
        let mut doc = Document::empty();
        let t = doc.text("Text.");
        let p = doc.paragraph(vec![t]);
        let env = doc.env("lemma", vec![p]);
        doc.blocks = vec![env];
        let inner = doc.blocks[0].kind.body().unwrap()[0].id;
        let json = serde_json::to_string(&doc).unwrap();
        let mut back: Document = serde_json::from_str(&json).unwrap();
        back.restore_ids();
        back.check_invariants().unwrap();
        assert_eq!(back.blocks[0].id, doc.blocks[0].id);
        assert_eq!(back.blocks[0].kind.body().unwrap()[0].id, inner);
    }

    /// The web address of a link, from its text or from text in a call in
    /// it; none outside links, for labels or local files.
    #[test]
    fn link_urls() {
        let mut doc = Document::empty();
        let word = doc.text("site");
        let word_id = word.id;
        let bold = doc.inline_call("strong", "", vec![word]);
        let web = doc.inline_call("link", r#""https://a.org/?q=\"x\"""#, vec![bold]);
        let (inner, label, file) = (doc.text("here"), doc.text("there"), doc.text("file"));
        let ids = (inner.id, label.id, file.id);
        let to_label = doc.inline_call("link", "<sec:intro>", vec![label]);
        let to_file = doc.inline_call("link", r#""file:///etc/passwd""#, vec![file]);
        let (t0, t1, t2, t3) = (
            doc.text("See "),
            doc.text(" "),
            doc.text(" "),
            doc.text("."),
        );
        let plain = t0.id;
        let mail = doc.inline_call("link", r#""mailto:a@b.org""#, vec![inner]);
        let t4 = doc.text(" ");
        let p = doc.paragraph(vec![t0, web, t1, to_label, t2, mail, t4, to_file, t3]);
        doc.blocks = vec![p];
        assert_eq!(
            doc.link_url(word_id).as_deref(),
            Some("https://a.org/?q=\"x\"")
        );
        assert_eq!(doc.link_url(ids.0).as_deref(), Some("mailto:a@b.org"));
        assert_eq!(doc.link_url(ids.1), None);
        assert_eq!(doc.link_url(ids.2), None);
        assert_eq!(doc.link_url(plain), None);
    }
}
