//! Typst source → tree: opening `.typ` files. Parsed with Typst's parser,
//! no compile. The definitions and rules before the first content become
//! the header; everything the tree can hold is mapped to it; the rest (code
//! in the middle of the document, constructs the editor does not know) is
//! kept as raw Typst blocks with their source verbatim, so nothing is lost
//! and the document renders as before. Unknown parts are kept whole: a
//! paragraph or formula holding anything the tree cannot represent stays
//! raw as a whole.

use model::{
    Block, BlockNode, Document, Fragment, Inline, InlineNode, ListKind, MathKind, MathNode,
    TitleFields, spaced_gap,
};
use typst::syntax::ast::{self, AstNode, MathTextKind};
use typst::syntax::{LinkedNode, SyntaxKind, SyntaxNode, parse};

use crate::serialize::{CETZ_IMPORT, OPERATORS, PREAMBLE, TITLE_AUTHORS, uses_cetz};
use crate::symbols::math_only;
use crate::{math_call_shape, resolve_symbol};

/// The document a `.typ` file holds.
pub fn import_typ(source: &str) -> Document {
    let source = source.replace("\r\n", "\n");
    // A file this editor exported starts with its own preamble.
    let body = source.strip_prefix(PREAMBLE).unwrap_or(&source);
    let root = parse(body);
    let root = LinkedNode::new(&root);
    let children: Vec<LinkedNode> = root.children().collect();
    let split = header_len(&children);
    let header_end = children[..split]
        .iter()
        .rev()
        .find(|n| !is_blank(n))
        .map_or(0, |n| n.range().end);

    let mut importer = Importer {
        doc: Document::empty(),
        src: body,
        sources: [body],
    };
    let blocks = importer.blocks(&children[split..]);
    let mut doc = importer.doc;
    doc.blocks = blocks;
    doc.normalize();

    let mut header = body[..header_end].to_string();
    // The export's title block: the metadata on the first line, the title
    // and author line as the first blocks.
    let first_line = header.split('\n').next().unwrap_or_default();
    let shown = ["#title()", TITLE_AUTHORS];
    let raw_title =
        |b: &BlockNode| matches!(&b.kind, Block::RawTypst(s) if shown.contains(&s.as_str()));
    let count = doc
        .blocks
        .iter()
        .take(2)
        .take_while(|b| raw_title(b))
        .count();
    if let (Some(fields), true) = (metadata_fields(first_line), count > 0) {
        header = header[first_line.len()..]
            .trim_start_matches('\n')
            .to_string();
        let title = doc.title_block(&fields.source());
        doc.blocks.splice(..count, [title]);
    }
    // The export adds the CeTZ import itself when there are diagrams.
    if doc.blocks.iter().any(|b| uses_cetz(b, true)) {
        header = header
            .lines()
            .filter(|l| *l != CETZ_IMPORT)
            .collect::<Vec<_>>()
            .join("\n");
    }
    if !header.trim().is_empty() {
        doc.header = header.trim_end().to_string() + "\n";
    }
    doc
}

/// The title block's fields from a line `#set document(title: […], author:
/// (…), date: datetime(…))` as the export writes it; `None` for anything
/// else.
fn metadata_fields(line: &str) -> Option<TitleFields> {
    let root = parse(line);
    let mut nodes = root.children().filter(|n| !n.kind().is_trivia());
    let (hash, rule) = (nodes.next()?, nodes.next()?);
    if hash.kind() != SyntaxKind::Hash || nodes.next().is_some() {
        return None;
    }
    let rule = rule.cast::<ast::SetRule>()?;
    let ast::Expr::Ident(target) = rule.target() else {
        return None;
    };
    if target.as_str() != "document" || rule.condition().is_some() {
        return None;
    }
    let mut fields = TitleFields::default();
    for arg in rule.args().items() {
        let ast::Arg::Named(named) = arg else {
            return None;
        };
        match (named.name().as_str(), named.expr()) {
            ("title", ast::Expr::ContentBlock(c)) => {
                let markup = c.body().to_untyped().full_text();
                fields.title = markup.to_string();
            }
            ("author", ast::Expr::Array(a)) => {
                for item in a.items() {
                    let ast::ArrayItem::Pos(ast::Expr::Str(s)) = item else {
                        return None;
                    };
                    fields.authors.push(s.get().to_string());
                }
            }
            ("date", ast::Expr::FuncCall(call)) => {
                let mut ymd = [0i64; 3];
                for arg in call.args().items() {
                    let ast::Arg::Named(n) = arg else {
                        return None;
                    };
                    let ast::Expr::Int(v) = n.expr() else {
                        return None;
                    };
                    let k = ["year", "month", "day"]
                        .iter()
                        .position(|k| *k == n.name().as_str())?;
                    ymd[k] = v.get();
                }
                let [y, m, d] = ymd;
                fields.date = format!("{y:04}-{m:02}-{d:02}");
            }
            _ => return None,
        }
    }
    // Only what the export writes for these fields.
    let written = crate::serialize::document_metadata(&fields);
    (written == line).then_some(fields)
}

/// Spaces and paragraph breaks.
fn is_blank(node: &SyntaxNode) -> bool {
    matches!(node.kind(), SyntaxKind::Space | SyntaxKind::Parbreak)
}

/// Code that defines things: what a code block (or the header) holds.
fn is_definition(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::LetBinding
            | SyntaxKind::SetRule
            | SyntaxKind::ShowRule
            | SyntaxKind::ModuleImport
    )
}

/// The length of a run of definitions starting at `nodes[0]`: `#let`s,
/// rules, imports and comments, with spaces between (and paragraph breaks,
/// if `parbreaks`). Trailing blanks are included.
fn definitions_len(nodes: &[LinkedNode], parbreaks: bool) -> usize {
    let mut i = 0;
    while let Some(n) = nodes.get(i) {
        match n.kind() {
            SyntaxKind::Space | SyntaxKind::Semicolon => {}
            SyntaxKind::Parbreak if parbreaks => {}
            SyntaxKind::LineComment | SyntaxKind::BlockComment => {}
            SyntaxKind::Hash if nodes.get(i + 1).is_some_and(|e| is_definition(e.kind())) => {
                i += 1;
            }
            _ => break,
        }
        i += 1;
    }
    i
}

/// The number of leading nodes that make the header.
fn header_len(nodes: &[LinkedNode]) -> usize {
    definitions_len(nodes, true)
}

/// The children of the markup inside a content block `[…]`.
fn content_markup<'a>(block: &LinkedNode<'a>) -> Vec<LinkedNode<'a>> {
    block
        .children()
        .find(|c| c.kind() == SyntaxKind::Markup)
        .map(|m| m.children().collect())
        .unwrap_or_default()
}

/// A function call's parts: its name (a plain identifier), the source
/// between its parentheses (trimmed), and its trailing content blocks.
fn call_parts<'a>(
    src: &str,
    call: &LinkedNode<'a>,
) -> Option<(String, String, Vec<LinkedNode<'a>>)> {
    let callee = call.children().next()?;
    if callee.kind() != SyntaxKind::Ident {
        return None;
    }
    let args = call.children().next_back()?;
    if args.kind() != SyntaxKind::Args {
        return None;
    }
    let (mut open, mut close) = (None, None);
    let mut contents = vec![];
    for child in args.children() {
        match child.kind() {
            SyntaxKind::LeftParen if open.is_none() => open = Some(child.range().end),
            SyntaxKind::RightParen if open.is_some() && close.is_none() => {
                close = Some(child.range().start)
            }
            SyntaxKind::ContentBlock if open.is_none() || close.is_some() => contents.push(child),
            _ => {}
        }
    }
    let inner = match (open, close) {
        (Some(a), Some(b)) => src[a..b].trim().to_string(),
        _ => String::new(),
    };
    Some((callee.leaf_text().to_string(), inner, contents))
}

/// An argument written without a name (not a paren, comma or trivia).
fn is_positional(kind: SyntaxKind) -> bool {
    !(kind.is_trivia()
        || matches!(
            kind,
            SyntaxKind::LeftParen
                | SyntaxKind::RightParen
                | SyntaxKind::Comma
                | SyntaxKind::Named
                | SyntaxKind::Spread
        ))
}

/// Block-level markup: what ends a paragraph.
fn is_block_level(node: &LinkedNode) -> bool {
    match node.kind() {
        SyntaxKind::Parbreak
        | SyntaxKind::Heading
        | SyntaxKind::ListItem
        | SyntaxKind::EnumItem
        | SyntaxKind::TermItem => true,
        SyntaxKind::Equation => node.cast::<ast::Equation>().is_some_and(|e| e.block()),
        SyntaxKind::Raw => node.cast::<ast::Raw>().is_some_and(|r| r.block()),
        _ => is_definition(node.kind()),
    }
}

/// A drawing in the editor's export form: `prefix` then a line
/// `import cetz.draw: *`, the drawing indented by two spaces, and `}))`.
fn drawing(code: &str, prefix: &str) -> Option<String> {
    let inner = code.strip_prefix(prefix)?.strip_suffix("}))")?;
    let inner = inner.strip_prefix('\n')?.strip_suffix('\n')?;
    let (first, body) = inner.split_once('\n').unwrap_or((inner, ""));
    if first.trim() != "import cetz.draw: *" {
        return None;
    }
    let body: Vec<&str> = body
        .split('\n')
        .map(|l| l.strip_prefix("  ").unwrap_or(l))
        .collect();
    Some(body.join("\n"))
}

/// The source of an embedded bibliography in the editor's form,
/// `bibliography(bytes(```bib …```.text))`.
fn bibliography_source(code: &str) -> Option<String> {
    let rest = code.strip_prefix("bibliography(bytes(")?;
    let ticks = rest.len() - rest.trim_start_matches('`').len();
    if ticks < 3 {
        return None;
    }
    let fence = &rest[..ticks];
    let inner = rest[ticks..].strip_prefix("bib\n")?;
    let end = format!("\n{fence}.text))");
    let source = inner.strip_suffix(end.as_str())?;
    (!source.contains(fence)).then(|| source.to_string())
}

struct Importer<'s> {
    doc: Document,
    src: &'s str,
    /// Where functions are defined, for telling calls from symbols in math.
    sources: [&'s str; 1],
}

impl Importer<'_> {
    /// A raw Typst block holding the source from `start` to `end`.
    fn raw(&mut self, start: usize, end: usize) -> BlockNode {
        let s = self.src[start..end].to_string();
        self.doc.raw(&s)
    }

    /// The blocks of a run of markup (the document, an environment's body,
    /// a list item's body).
    fn blocks(&mut self, nodes: &[LinkedNode]) -> Vec<BlockNode> {
        let mut out: Vec<BlockNode> = vec![];
        let mut run: Vec<LinkedNode> = vec![];
        // Where the source of the last block ends, for labels after raw
        // blocks.
        let mut last_end = 0;
        let mut i = 0;
        while i < nodes.len() {
            let n = &nodes[i];
            let blank_run = run.iter().all(|r| is_blank(r));
            match n.kind() {
                SyntaxKind::Parbreak => self.flush(&mut run, &mut out, &mut last_end),
                SyntaxKind::Label if blank_run && !out.is_empty() => {
                    let label = n.cast::<ast::Label>().map(|l| l.get().to_string());
                    let last = out.last_mut().expect("not empty");
                    let free = last.kind.labelable() && last.label.is_empty();
                    match (&mut last.kind, label) {
                        (_, Some(l)) if free => last.label = l,
                        (model::Block::RawTypst(s), _) => {
                            s.push_str(&self.src[last_end..n.range().end]);
                        }
                        _ => {
                            let b = self.raw(n.range().start, n.range().end);
                            out.push(b);
                        }
                    }
                    last_end = n.range().end;
                    run.clear();
                }
                SyntaxKind::Hash | SyntaxKind::LineComment | SyntaxKind::BlockComment
                    if blank_run && definitions_len(&nodes[i..], false) > 0 =>
                {
                    let len = definitions_len(&nodes[i..], false);
                    let end = nodes[i..i + len]
                        .iter()
                        .rev()
                        .find(|c| !is_blank(c))
                        .map_or(n.range().end, |c| c.range().end);
                    run.clear();
                    let b = self.raw(n.range().start, end);
                    out.push(b);
                    last_end = end;
                    i += len;
                    continue;
                }
                // A rule in a paragraph (after its `#`) stays in it.
                _ if is_block_level(n) && !is_definition(n.kind()) => {
                    self.flush(&mut run, &mut out, &mut last_end);
                    let b = self.block(n);
                    out.push(b);
                    last_end = n.range().end;
                }
                // A block call on lines of its own (`#theorem[…]` right under
                // a line of text) ends the paragraph, as it does in Typst.
                SyntaxKind::Hash if on_own_lines(&run, nodes, i) => {
                    let call = &nodes[i + 1];
                    match self.block_call(call) {
                        Some(b) => {
                            self.flush(&mut run, &mut out, &mut last_end);
                            out.push(b);
                            last_end = call.range().end;
                            i += 1;
                        }
                        // One the tree cannot hold (`#lemma(statement())`)
                        // is a block too when it is labelled: raw on its own,
                        // the text before it stays editable.
                        None if labelled(nodes, i + 1) => {
                            self.flush(&mut run, &mut out, &mut last_end);
                            let b = self.raw(n.range().start, call.range().end);
                            out.push(b);
                            last_end = call.range().end;
                            i += 1;
                        }
                        None => run.push(n.clone()),
                    }
                }
                _ => run.push(n.clone()),
            }
            i += 1;
        }
        self.flush(&mut run, &mut out, &mut last_end);
        out
    }

    /// A block-level element: heading, list item, display formula, code
    /// listing; raw if its content cannot be represented.
    fn block(&mut self, n: &LinkedNode) -> BlockNode {
        let made = match n.kind() {
            SyntaxKind::Heading => self.heading(n),
            SyntaxKind::ListItem | SyntaxKind::EnumItem | SyntaxKind::TermItem => self.item(n),
            SyntaxKind::Equation => n
                .cast::<ast::Equation>()
                .and_then(|eq| self.math(eq.body().to_untyped()))
                .map(|nodes| self.doc.display(nodes)),
            _ => None,
        };
        made.unwrap_or_else(|| self.raw(n.range().start, n.range().end))
    }

    fn heading(&mut self, n: &LinkedNode) -> Option<BlockNode> {
        let depth = n.cast::<ast::Heading>()?.depth().get();
        let mut body: Vec<LinkedNode> = n
            .children()
            .find(|c| c.kind() == SyntaxKind::Markup)?
            .children()
            .collect();
        while body.last().is_some_and(|c| is_blank(c)) {
            body.pop();
        }
        // `= Title <label>`: the label is in the heading's markup.
        let label = match body.last() {
            Some(l) if l.kind() == SyntaxKind::Label => {
                let label = l.cast::<ast::Label>()?.get().to_string();
                body.pop();
                label
            }
            _ => String::new(),
        };
        let inlines = self.inlines(&trimmed(&body))?;
        let mut block = self.doc.heading(u8::try_from(depth).ok()?, inlines);
        block.label = label;
        Some(block)
    }

    fn item(&mut self, n: &LinkedNode) -> Option<BlockNode> {
        let markups: Vec<LinkedNode> = n
            .children()
            .filter(|c| c.kind() == SyntaxKind::Markup)
            .collect();
        let (kind, term) = match n.kind() {
            SyntaxKind::ListItem => (ListKind::Bullet, None),
            SyntaxKind::EnumItem => {
                // Only automatic numbers: the tree has no explicit ones.
                if n.cast::<ast::EnumItem>()?.number().is_some() {
                    return None;
                }
                (ListKind::Numbered, None)
            }
            _ => {
                let term: Vec<LinkedNode> = markups.first()?.children().collect();
                (ListKind::Term, Some(self.inlines(&trimmed(&term))?))
            }
        };
        let body: Vec<LinkedNode> = markups.last()?.children().collect();
        let body = self.blocks(&body);
        let mut block = self.doc.item(kind, body);
        if let (Some(term), model::Block::Item { term: t, .. }) = (term, &mut block.kind) {
            *t = self.doc.normalized_inlines(term);
        }
        Some(block)
    }

    /// End the paragraph collected in `run`: a block call standing alone, a
    /// paragraph, or raw Typst.
    fn flush(&mut self, run: &mut Vec<LinkedNode>, out: &mut Vec<BlockNode>, last_end: &mut usize) {
        let nodes = trimmed(run);
        run.clear();
        let (Some(first), Some(last)) = (nodes.first(), nodes.last()) else {
            return;
        };
        let (start, end) = (first.range().start, last.range().end);
        let block = self
            .paragraph(&nodes)
            .unwrap_or_else(|| self.raw(start, end));
        out.push(block);
        *last_end = end;
    }

    /// A paragraph's nodes as a block: a call standing alone (with a label
    /// after it) may be an environment, a figure, a diagram or the
    /// bibliography; else a paragraph.
    fn paragraph(&mut self, nodes: &[LinkedNode]) -> Option<BlockNode> {
        let mut rest = nodes;
        let mut label = String::new();
        if let [before @ .., l] = rest
            && l.kind() == SyntaxKind::Label
        {
            label = l.cast::<ast::Label>()?.get().to_string();
            rest = trimmed_slice(before);
        }
        let lone_call = match rest {
            [hash, call] | [hash, call, _]
                if hash.kind() == SyntaxKind::Hash
                    && call.kind() == SyntaxKind::FuncCall
                    && rest
                        .get(2)
                        .is_none_or(|s| s.kind() == SyntaxKind::Semicolon) =>
            {
                Some(call)
            }
            _ => None,
        };
        let block = match lone_call.and_then(|c| self.block_call(c)) {
            Some(b) => b,
            None if label.is_empty() => {
                let inlines = self.inlines(rest)?;
                self.doc.paragraph(inlines)
            }
            None => return None,
        };
        if !label.is_empty() {
            if !block.kind.labelable() {
                return None;
            }
            let mut block = block;
            block.label = label;
            return Some(block);
        }
        Some(block)
    }

    /// A call standing alone as a paragraph, as a block: a diagram, the
    /// bibliography, a figure, or an environment (a call whose body holds
    /// blocks or starts on a new line, or that has a title). `None` for a
    /// call that reads as a paragraph holding one inline call.
    fn block_call(&mut self, call: &LinkedNode) -> Option<BlockNode> {
        let code = call.full_text();
        if let Some(src) = drawing(&code, "align(center, cetz.canvas({") {
            return Some(self.doc.diagram(&src));
        }
        if let Some(src) = bibliography_source(&code) {
            return Some(self.doc.bibliography_block(&src));
        }
        let (name, mut args, contents) = call_parts(self.src, call)?;
        if name == "figure" && contents.is_empty() {
            return self.code_figure(call);
        }
        if contents.is_empty() || contents.len() > 2 {
            return None;
        }
        let body = content_markup(contents.last()?);
        let starts_on_new_line = body
            .first()
            .is_some_and(|c| c.kind() == SyntaxKind::Space && c.leaf_text().contains('\n'));
        let blocks_inside = body.iter().any(is_block_level);
        if !(name == "figure" || contents.len() == 2 || starts_on_new_line || blocks_inside) {
            return None;
        }
        let caption = if name == "figure" {
            let (rest, caption) = self.caption_arg(call)?;
            args = rest;
            caption
        } else {
            None
        };
        let title = match &contents[..] {
            [title, _] => Some(self.inlines(&trimmed(&content_markup(title)))?),
            _ => None,
        };
        let mut body = self.blocks(&body);
        if let Some(caption) = caption {
            body.push(self.doc.caption(caption));
        }
        let mut block = self.doc.env(&name, body);
        if let model::Block::Env {
            args: a, title: t, ..
        } = &mut block.kind
        {
            *a = args;
            if let Some(title) = title {
                *t = self.doc.normalized_inlines(title);
            }
        }
        Some(block)
    }

    /// A figure whose body is its first argument, `#figure(dia(…), caption:
    /// […])`: content in brackets becomes its blocks, code a raw Typst block.
    fn code_figure(&mut self, call: &LinkedNode) -> Option<BlockNode> {
        let args = call.children().next_back()?;
        let value = args.children().find(|c| is_positional(c.kind()))?;
        let mut body = if value.kind() == SyntaxKind::ContentBlock {
            self.blocks(&content_markup(&value))
        } else {
            vec![self.doc.raw(&format!("#{}", value.full_text()))]
        };
        let (rest, caption) = self.caption_arg(call)?;
        if let Some(caption) = caption {
            body.push(self.doc.caption(caption));
        }
        let mut block = self.doc.env("figure", body);
        if let model::Block::Env { args, .. } = &mut block.kind {
            *args = rest;
        }
        Some(block)
    }

    /// A figure's `caption: […]` argument, taken out of its arguments (with
    /// its body, the first positional one, if written in the parentheses):
    /// the other arguments' source and the caption's inlines (if it has one).
    fn caption_arg(&mut self, call: &LinkedNode) -> Option<(String, Option<Vec<InlineNode>>)> {
        let args = call.children().next_back()?;
        let mut rest = vec![];
        let mut caption = None;
        let body = args
            .children()
            .find(|c| is_positional(c.kind()))
            .map(|c| c.range());
        for child in args.children() {
            if Some(child.range()) == body {
                continue;
            }
            match child.kind() {
                SyntaxKind::Named => {
                    let named = child.cast::<ast::Named>()?;
                    let value = child.children().next_back()?;
                    if named.name().as_str() == "caption"
                        && value.kind() == SyntaxKind::ContentBlock
                    {
                        caption = Some(self.inlines(&trimmed(&content_markup(&value)))?);
                    } else {
                        rest.push(child.full_text().to_string());
                    }
                }
                SyntaxKind::ContentBlock => {} // the body
                k if k.is_trivia()
                    || matches!(
                        k,
                        SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Comma
                    ) => {}
                _ => rest.push(child.full_text().to_string()),
            }
        }
        Some((rest.join(", "), caption))
    }

    /// The inlines of a paragraph (or heading, title, call body), or `None`
    /// if it holds anything the tree cannot represent.
    fn inlines(&mut self, nodes: &[LinkedNode]) -> Option<Vec<InlineNode>> {
        let mut out = vec![];
        let mut text = String::new();
        let mut i = 0;
        while i < nodes.len() {
            let n = &nodes[i];
            let mut inline = None;
            match n.kind() {
                SyntaxKind::Text => text.push_str(n.leaf_text()),
                // A space after a line break is how the export writes it.
                SyntaxKind::Space if text.ends_with('\n') => {}
                SyntaxKind::Space => text.push(' '),
                SyntaxKind::Linebreak => text.push('\n'),
                SyntaxKind::Escape => text.push(n.cast::<ast::Escape>()?.get()),
                SyntaxKind::Shorthand => text.push(n.cast::<ast::Shorthand>()?.get()),
                SyntaxKind::SmartQuote => text.push_str(n.leaf_text()),
                SyntaxKind::Strong | SyntaxKind::Emph => {
                    let body: Vec<LinkedNode> = n
                        .children()
                        .find(|c| c.kind() == SyntaxKind::Markup)?
                        .children()
                        .collect();
                    let body = self.inlines(&body)?;
                    let name = if n.kind() == SyntaxKind::Strong {
                        "strong"
                    } else {
                        "emph"
                    };
                    inline = Some(self.doc.inline_call(name, "", body));
                }
                SyntaxKind::Raw => {
                    let raw = n.cast::<ast::Raw>()?;
                    let single = n.children().next().is_some_and(|d| d.leaf_text() == "`");
                    if raw.block() || raw.lang().is_some() || !single {
                        return None;
                    }
                    let lines: Vec<&str> = raw.lines().map(|t| t.get().as_str()).collect();
                    inline = Some(self.doc.raw_inline(&lines.join("\n")));
                }
                SyntaxKind::Link => {
                    let url = n.cast::<ast::Link>()?.get().to_string();
                    let body = vec![self.doc.text(&url)];
                    inline = Some(self.doc.inline_call(
                        "link",
                        &crate::serialize::quoted(&url),
                        body,
                    ));
                }
                SyntaxKind::Ref => {
                    let r = n.cast::<ast::Ref>()?;
                    if r.supplement().is_some() {
                        return None;
                    }
                    inline = Some(self.doc.reference(r.target()));
                }
                SyntaxKind::Equation => {
                    let eq = n.cast::<ast::Equation>()?;
                    if eq.block() {
                        return None;
                    }
                    let nodes = self.math(eq.body().to_untyped())?;
                    inline = Some(self.doc.math(nodes));
                }
                SyntaxKind::Hash => {
                    let call = nodes
                        .get(i + 1)
                        .filter(|c| c.kind() == SyntaxKind::FuncCall)?;
                    let (name, args, contents) = call_parts(self.src, call)?;
                    let [content] = &contents[..] else {
                        return None;
                    };
                    let body = self.inlines(&content_markup(content))?;
                    inline = Some(self.doc.inline_call(&name, &args, body));
                    i += 1;
                    if nodes
                        .get(i + 1)
                        .is_some_and(|s| s.kind() == SyntaxKind::Semicolon)
                    {
                        i += 1;
                    }
                }
                _ => return None,
            }
            if let Some(inline) = inline {
                let t = self.doc.text(&std::mem::take(&mut text));
                out.push(t);
                out.push(inline);
            }
            i += 1;
        }
        let t = self.doc.text(&text);
        out.push(t);
        Some(out)
    }

    /// The nodes of a formula, or `None` if it holds anything the tree
    /// cannot represent.
    fn math(&mut self, node: &SyntaxNode) -> Option<Vec<MathNode>> {
        let mut out = vec![];
        if node.kind() == SyntaxKind::Math {
            let children: Vec<&SyntaxNode> = node.children().collect();
            let mut i = 0;
            let mut space = false;
            while i < children.len() {
                let n = children[i];
                let start = out.len();
                match n.kind() {
                    // Kept where it shows, next to a bar or quoted text.
                    SyntaxKind::Space => space = true,
                    // Parentheses that only group (a fraction part, script
                    // or radicand): Typst does not show them.
                    SyntaxKind::LeftParen | SyntaxKind::RightParen => {}
                    SyntaxKind::Hash => {
                        let code = children.get(i + 1)?;
                        let text = code.full_text();
                        let atom =
                            match drawing(&text, "box(baseline: 30%, cetz.canvas(length: 1em, {") {
                                Some(src) => MathKind::Diagram(src),
                                None => MathKind::Code(text.to_string()),
                            };
                        out.push(self.doc.node(atom));
                        i += 1;
                        if children
                            .get(i + 1)
                            .is_some_and(|s| s.kind() == SyntaxKind::Semicolon)
                        {
                            i += 1;
                        }
                    }
                    _ => self.math_expr(n, &mut out)?,
                }
                if out.len() > start {
                    if space && start > 0 && spaced_gap(&out[start - 1], &out[start]) {
                        out[start].space = true;
                    }
                    space = false;
                }
                i += 1;
            }
        } else {
            self.math_expr(node, &mut out)?;
        }
        Some(out)
    }

    fn sym(&mut self, s: &str) -> MathNode {
        self.doc.sym(s)
    }

    /// A name in math: an operator (`log`), a symbol (`alpha`,
    /// `arrow.r`), or anything else (`dif`, a header definition), kept as
    /// code.
    fn name(&mut self, name: &str) -> MathNode {
        if OPERATORS.iter().any(|(op, ..)| *op == name) {
            self.doc.ident(name)
        } else if let Some(s) = resolve_symbol(name) {
            self.doc.sym(&s)
        } else if math_only(name.split('.').next().unwrap_or(name)) {
            self.doc.code(&format!("math.{name}"))
        } else {
            self.doc.code(name)
        }
    }

    fn math_expr(&mut self, n: &SyntaxNode, out: &mut Vec<MathNode>) -> Option<()> {
        match n.kind() {
            SyntaxKind::Math => out.extend(self.math(n)?),
            SyntaxKind::MathText => {
                let node = match n.cast::<ast::MathText>()?.get() {
                    MathTextKind::Number(s) => self.doc.num(s),
                    MathTextKind::Grapheme(g) => {
                        let mut chars = g.chars();
                        match (chars.next(), chars.next()) {
                            (Some(c), None) if c.is_alphabetic() => self.doc.ident(g),
                            _ => self.sym(g),
                        }
                    }
                };
                out.push(node);
            }
            SyntaxKind::MathIdent | SyntaxKind::MathFieldAccess => {
                let name = n.full_text();
                let node = self.name(&name);
                out.push(node);
            }
            // `->` is stored as `→`, as typed; a single char (`-`, `*`) as
            // itself, which the export writes back.
            SyntaxKind::MathShorthand => {
                let text = n.leaf_text();
                let node = if text.chars().count() == 1 {
                    self.sym(text)
                } else {
                    let c = ast::MathShorthand::LIST.iter().find(|(s, _)| s == text)?.1;
                    self.sym(&c.to_string())
                };
                out.push(node);
            }
            SyntaxKind::Comma
            | SyntaxKind::Semicolon
            | SyntaxKind::Dot
            | SyntaxKind::Bang
            | SyntaxKind::Colon => {
                let node = self.sym(n.leaf_text());
                out.push(node);
            }
            // An escaped bracket (`\{`) does not pair up and stretch; the
            // tree's brackets do, so it is kept as the named symbol.
            SyntaxKind::Escape => {
                let c = n.cast::<ast::Escape>()?.get();
                let name = match c {
                    '(' => Some("paren.l"),
                    ')' => Some("paren.r"),
                    '[' => Some("bracket.l"),
                    ']' => Some("bracket.r"),
                    '{' => Some("brace.l"),
                    '}' => Some("brace.r"),
                    _ => None,
                };
                let node = match name {
                    Some(name) => self.doc.code(&format!("sym.{name}")),
                    None => self.sym(&c.to_string()),
                };
                out.push(node);
            }
            SyntaxKind::Str => {
                let s = n.cast::<ast::Str>()?.get();
                out.push(self.doc.node(MathKind::Text(s.to_string())));
            }
            SyntaxKind::MathAlignPoint => out.push(self.doc.node(MathKind::Align)),
            SyntaxKind::Linebreak => out.push(self.doc.node(MathKind::Linebreak)),
            SyntaxKind::MathPrimes => {
                let count = n.cast::<ast::MathPrimes>()?.count();
                out.push(self.doc.node(MathKind::Primes(count)));
            }
            SyntaxKind::MathDelimited => {
                let d = n.cast::<ast::MathDelimited>()?;
                let open = d.open().to_untyped().full_text();
                let close = d.close().to_untyped().full_text();
                if open.chars().count() != 1 || close.chars().count() != 1 {
                    return None;
                }
                let body = self.math(d.body().to_untyped())?;
                let open = self.sym(&open);
                let close = self.sym(&close);
                out.push(open);
                out.extend(body);
                out.push(close);
            }
            SyntaxKind::MathAttach => {
                let a = n.cast::<ast::MathAttach>()?;
                let mut base = self.math(a.base().to_untyped())?;
                if let Some(p) = a.primes() {
                    base.push(self.doc.node(MathKind::Primes(p.count())));
                }
                let sup = match a.top() {
                    Some(t) => Some(self.math(t.to_untyped())?),
                    None => None,
                };
                let sub = match a.bottom() {
                    Some(b) => Some(self.math(b.to_untyped())?),
                    None => None,
                };
                if sup.is_none() && sub.is_none() {
                    out.extend(base);
                } else {
                    out.push(self.doc.scripts(base, sup, sub));
                }
            }
            SyntaxKind::MathFrac => {
                let f = n.cast::<ast::MathFrac>()?;
                let num = self.math(f.num().to_untyped())?;
                let den = self.math(f.denom().to_untyped())?;
                out.push(self.doc.frac(num, den));
            }
            SyntaxKind::MathRoot => {
                let r = n.cast::<ast::MathRoot>()?;
                let radicand = self.math(r.radicand().to_untyped())?;
                let radicand = self.doc.row(radicand);
                let node = match r.index() {
                    None => self.doc.call("sqrt", vec![radicand]),
                    Some(i) => {
                        let index = vec![self.doc.num(&i.to_string())];
                        let index = self.doc.row(index);
                        self.doc.call("root", vec![index, radicand])
                    }
                };
                out.push(node);
            }
            SyntaxKind::MathCall => self.math_call(n, out)?,
            _ => return None,
        }
        Some(())
    }

    /// A call in math, `sqrt(x)`, `cal(A)`, `ket(psi)`: a call node with
    /// one row per argument. A symbol that cannot be called (`phi(x)`) is
    /// the symbol followed by its parenthesized arguments, as Typst shows
    /// it.
    fn math_call(&mut self, n: &SyntaxNode, out: &mut Vec<MathNode>) -> Option<()> {
        let call = n.cast::<ast::MathCall>()?;
        let name = call.callee().to_untyped().full_text().to_string();
        let callable =
            resolve_symbol(&name).is_none() || math_call_shape(&self.sources, &name).is_some();
        if !callable {
            let node = self.name(&name);
            out.push(node);
            for item in call.args().content_items() {
                match item {
                    ast::MathArgItem::Arg(ast::Arg::Pos(e)) => {
                        out.extend(self.math(e.to_untyped())?)
                    }
                    ast::MathArgItem::Arg(_) => return None,
                    ast::MathArgItem::Space(_) => {}
                    ast::MathArgItem::Comma(c, _)
                    | ast::MathArgItem::Semicolon(c, _)
                    | ast::MathArgItem::LeftParen(c, _)
                    | ast::MathArgItem::RightParen(c, _) => {
                        let node = self.sym(&c.to_string());
                        out.push(node);
                    }
                }
            }
            return Some(());
        }
        let mut args = vec![];
        let mut opts = vec![];
        for arg in call.args().arg_items() {
            if let ast::Arg::Named(named) = arg.arg {
                opts.push(code_opt(named)?);
                continue;
            }
            let ast::Arg::Pos(e) = arg.arg else {
                return None;
            };
            if arg.ends_in_semicolon {
                return None;
            }
            let row = self.math(e.to_untyped())?;
            args.push(self.doc.row(row));
        }
        out.push(self.doc.call_with(&name, &opts.join(", "), args));
        Some(())
    }
}

/// A named argument of a math call as code (`size: #55%` gives
/// `size: 55%`): a value after `#`, or a string. `None` for math content
/// (`attach(x, t: y)`), which is not an option.
fn code_opt(named: ast::Named) -> Option<String> {
    let text = named.to_untyped().full_text();
    let (_, value) = text.split_once(':')?;
    let value = value.trim();
    let code = match value.strip_prefix('#') {
        Some(code) => code,
        None if matches!(named.expr(), ast::Expr::Str(_)) => value,
        None => return None,
    };
    Some(format!("{}: {code}", named.name().as_str()))
}

/// Pasted Typst as a fragment. In a formula (`in_math`) it is read as
/// math (`$…$`); if it is not a formula, as text to type. Else it is read
/// as a document's content, after a first paragraph so leading rules and
/// definitions are content, not a header: one paragraph gives inlines,
/// anything else blocks.
pub fn parse_fragment(text: &str, in_math: bool) -> Fragment {
    if in_math {
        let doc = import_typ(&format!("${}$", text.trim()));
        if let [block] = &doc.blocks[..]
            && let Block::Paragraph(inlines) = &block.kind
            && let [_, m, _] = &inlines[..]
            && let Inline::Math(row) = &m.kind
        {
            return Fragment::Math(row.nodes.clone());
        }
        return Fragment::Plain(text.to_string());
    }
    let mut blocks = import_typ(&format!("x\n\n{text}")).blocks;
    if !blocks.is_empty() {
        blocks.remove(0);
    }
    match &blocks[..] {
        [block] if block.label.is_empty() => match &block.kind {
            Block::Paragraph(inlines) => Fragment::Inlines(inlines.clone()),
            _ => Fragment::Blocks(blocks),
        },
        _ => Fragment::Blocks(blocks),
    }
}

/// Options written as in math (`size: #55%`) as code (`size: 55%`); as
/// they are if they are not all options.
pub fn code_opts(math: &str) -> String {
    fn find(n: &SyntaxNode) -> Option<ast::MathCall<'_>> {
        n.cast().or_else(|| n.children().find_map(find))
    }
    let root = parse(&format!("$ff({math})$"));
    let opts = find(&root).and_then(|c| {
        c.args()
            .arg_items()
            .map(|item| match item.arg {
                ast::Arg::Named(named) => code_opt(named),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
    });
    opts.map_or_else(|| math.to_string(), |o| o.join(", "))
}

/// Whether `nodes[i]` is the `#` of a call that stands on lines of its own
/// in the paragraph collected in `run`: a line break (or nothing) before it,
/// and after it a line break, a paragraph break or the end, possibly after
/// a label.
fn on_own_lines(run: &[LinkedNode], nodes: &[LinkedNode], i: usize) -> bool {
    let breaks = |n: &LinkedNode| {
        n.kind() == SyntaxKind::Parbreak
            || n.kind() == SyntaxKind::Space && n.leaf_text().contains('\n')
    };
    let before = run.iter().all(|r| is_blank(r)) || run.last().is_some_and(breaks);
    if !before
        || nodes
            .get(i + 1)
            .is_none_or(|c| c.kind() != SyntaxKind::FuncCall)
    {
        return false;
    }
    let mut rest = nodes[i + 2..].iter().peekable();
    // A label after it, `] <thm:main>`.
    if rest
        .peek()
        .is_some_and(|n| n.kind() == SyntaxKind::Space && !breaks(n))
        && nodes
            .get(i + 3)
            .is_some_and(|n| n.kind() == SyntaxKind::Label)
    {
        rest.next();
    }
    if rest.peek().is_some_and(|n| n.kind() == SyntaxKind::Label) {
        rest.next();
    }
    rest.next().is_none_or(breaks)
}

/// Whether a label follows `nodes[k]` (after a space on the same line).
fn labelled(nodes: &[LinkedNode], k: usize) -> bool {
    match (nodes.get(k + 1), nodes.get(k + 2)) {
        (Some(l), _) if l.kind() == SyntaxKind::Label => true,
        (Some(s), Some(l)) => {
            s.kind() == SyntaxKind::Space
                && !s.leaf_text().contains('\n')
                && l.kind() == SyntaxKind::Label
        }
        _ => false,
    }
}

/// Without leading and trailing spaces.
fn trimmed<'a>(nodes: &[LinkedNode<'a>]) -> Vec<LinkedNode<'a>> {
    trimmed_slice(nodes).to_vec()
}

fn trimmed_slice<'n, 'a>(nodes: &'n [LinkedNode<'a>]) -> &'n [LinkedNode<'a>] {
    let start = nodes
        .iter()
        .position(|n| !is_blank(n))
        .unwrap_or(nodes.len());
    let end = nodes
        .iter()
        .rposition(|n| !is_blank(n))
        .map_or(start, |e| e + 1);
    &nodes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BridgeWorld, compile, render_page, serialize_document};
    use model::Block;

    /// The pages of `source` as images.
    fn pages(source: &str) -> Vec<Vec<u8>> {
        let mut world = BridgeWorld::new();
        let doc = compile(&mut world, source).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
        doc.pages()
            .iter()
            .map(|p| render_page(p, 2.0).data)
            .collect()
    }

    /// Importing then exporting renders the same pages (the original with
    /// the editor's preamble, which the export adds).
    fn renders_the_same(source: &str) -> Document {
        let doc = import_typ(source);
        doc.check_invariants().unwrap();
        let exported = serialize_document(&doc);
        let before = pages(&format!("{PREAMBLE}{source}"));
        let after = pages(&exported);
        assert_eq!(before.len(), after.len(), "page count\n{exported}");
        for (i, (a, b)) in before.iter().zip(&after).enumerate() {
            assert!(a == b, "page {i} differs\n--- exported:\n{exported}");
        }
        doc
    }

    const PAPER: &str = r#"#set heading(numbering: "1.1")
#set math.equation(numbering: "(1)")
#let theorem(body) = block(stroke: 0.5pt, inset: 4pt)[*Theorem.* #body]
#let ket(x) = $lr(|#x chevron.r)$

= Introduction <intro>

Some *bold* and _italic_ text with $x^2 + y_1 = alpha/2$ and `code`, a link
https://typst.org and #link("https://example.com")[a link]. See @intro and @eq:a.

$ sum_(i=1)^n f'(x_i) = integral_0^1 sqrt(x) dif x + ket(psi) $ <eq:a>

- first item
- second with $a <= b$
  + nested

/ Term: description

#theorem[
  A theorem body with $phi(x)$.

  Second paragraph.
]

#set heading(numbering: "A.1")

= Appendix

Text with a footnote#footnote[The note.] and a line \ break -- dash.

$ f(x) &= cases(1 "if" x > 0, 0) \
  &= (a+b)^2 / (c d) $

#figure(caption: [A caption])[
  #rect(width: 2cm)
]

#h(1cm) raw paragraph.
"#;

    #[test]
    fn a_paper_imports_and_renders_the_same() {
        let doc = renders_the_same(PAPER);
        assert!(doc.header.contains("#let ket"), "{}", doc.header);
        let kinds: Vec<&str> = doc.blocks.iter().map(|b| b.label()).collect();
        let raw = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, Block::RawTypst(_)))
            .count();
        assert_eq!(
            raw, 2,
            "the mid-document rule and the h(1cm) paragraph: {kinds:?}"
        );
        assert_eq!(doc.blocks[0].label, "intro");
        let labels: Vec<&str> = doc.labels().into_iter().map(|(l, _)| l).collect();
        assert_eq!(labels, ["intro", "eq:a"]);
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(&b.kind, Block::Env { name, .. } if name == "theorem"))
        );
        let figure = doc
            .blocks
            .iter()
            .find(|b| matches!(&b.kind, Block::Env { name, .. } if name == "figure"))
            .expect("a figure");
        let body = figure.kind.body().unwrap();
        assert!(matches!(body.last().unwrap().kind, Block::Caption(_)));
    }

    #[test]
    fn a_figure_with_code_for_its_body_is_a_figure() {
        let source = "#figure(\n  rect(width: 2cm),\n  caption: [A box.],\n) <fig:box>\n\n#figure({\n  let w = 1cm\n  rect(width: w)\n}, caption: [Code.]) <fig:code>\n\nSee @fig:box and @fig:code.\n";
        let doc = renders_the_same(source);
        for (block, label) in doc.blocks.iter().zip(["fig:box", "fig:code"]) {
            assert!(
                matches!(&block.kind, Block::Env { name, args, .. } if name == "figure" && args.is_empty()),
                "{:?}",
                block.kind
            );
            assert_eq!(block.label, label);
            let body = block.kind.body().unwrap();
            assert!(matches!(body[0].kind, Block::RawTypst(_)));
            assert!(matches!(body[1].kind, Block::Caption(_)));
        }
    }

    #[test]
    fn formulas() {
        for f in [
            "$ a/b + (x+1)/2 $",
            "$ x_1^2 + x^(n+1) + (a+b)^2 $",
            "$ f'' + g'^2 $",
            "$ sqrt(x) + root(3, y) + √(z+1) $",
            "$ cal(A) hat(x) overline(a b) $",
            "$ phi(x) + sin x + lim_(n -> oo) a_n $",
            "$ [0, 1) $",
            "$ \\{h, overline(h)\\} $",
            "$ {x | x > 0} + abs(y) $",
            "$ a <= b != c -> d $",
            "$ x \" text\" y $",
            "$ \"Ker\"(A) + \"Rep\" x $",
            "$ dif x + dots.h + arrow.r.long $",
            "$ #h(1em) x $",
        ] {
            let doc = renders_the_same(f);
            assert!(
                matches!(doc.blocks[0].kind, Block::DisplayMath(_)),
                "{f}: {:?}",
                doc.blocks[0].kind
            );
        }
    }

    /// A call standing alone is an environment when its body holds blocks,
    /// starts on a new line or has a title; else a paragraph with the call.
    #[test]
    fn lone_calls() {
        let header = "#let lemma(..args) = block(args.pos().last())\n";
        let kind = |body: &str| {
            let doc = renders_the_same(&format!("{header}{body}"));
            doc.blocks[0].label()
        };
        assert_eq!(kind("#highlight[short]"), "paragraph");
        assert_eq!(kind("#lemma[\n  Body.\n]"), "environment");
        assert_eq!(kind("#lemma[Title][Body.]"), "environment");
        assert_eq!(kind("#lemma[One.\n\nTwo.]"), "environment");
        assert_eq!(kind("+ one\n+ two"), "list item");
        assert_eq!(kind("3. three"), "raw typst");

        // On lines of its own under a line of text: the paragraph ends, the
        // environment keeps its label.
        let doc = renders_the_same(&format!(
            "{header}We prove\n#lemma[Title][\n  Body.\n] <lem:a>\nwhere more text follows.\n\n\
             Text and\n#highlight[an inline call]\non its own line."
        ));
        let kinds: Vec<&str> = doc.blocks.iter().map(|b| b.label()).collect();
        assert_eq!(
            kinds,
            ["paragraph", "environment", "paragraph", "paragraph"]
        );
        assert_eq!(doc.blocks[1].label, "lem:a");

        // A labelled call with no body: raw on its own, label included.
        let doc = renders_the_same(&format!(
            "{header}#let stmt() = [Statement.]\n\nAs follows:\n  #lemma(stmt()) <lem:b>\n"
        ));
        let kinds: Vec<&str> = doc.blocks.iter().map(|b| b.label()).collect();
        assert_eq!(kinds, ["paragraph", "raw typst"]);
        assert!(matches!(&doc.blocks[1].kind, Block::RawTypst(s) if s == "#lemma(stmt()) <lem:b>"));
    }

    /// Spaces next to bars and quoted text show: they are kept as written,
    /// so such formulas import (the user's paper: `z_(|partial v|)`).
    #[test]
    fn bars_and_text_keep_their_spaces() {
        for src in [
            "$ |x^2| $",
            "$ \"Rep\" (A) + \"Ker\"(B) $",
            "$z_(|partial v|-1)$ and $3 |partial v| - 1$",
            "$deg v = |∂ v|$, ${h | b}$, $p| q$ and $a |b$",
            "$Delta^(|∂ P_i|-2)(d) dot z_(|x_(i+1)|-1)$",
            "$|x||y| + |[a]| + |-x| + |=|$",
            "$x \"if\"y$ and $x\"if\" y$",
        ] {
            let doc = renders_the_same(src);
            assert!(
                matches!(
                    doc.blocks[0].kind,
                    Block::Paragraph(_) | Block::DisplayMath(_)
                ),
                "{src}"
            );
        }
    }

    #[test]
    fn unknown_parts_stay_raw() {
        for src in [
            "$ mat(1, 2; 3, 4) $",
            "Text #h(1em) more",
            "#lorem(5)",
            "Text // comment",
            "#v(1em)\n#set text(red)\nText",
        ] {
            let doc = renders_the_same(src);
            assert!(matches!(doc.blocks[0].kind, Block::RawTypst(_)), "{src}");
        }
    }

    /// Pasted Typst: a paragraph's text as inlines, several blocks (also
    /// rules, which would be a file's header) as blocks, math as nodes.
    #[test]
    fn pasted_typst() {
        let Fragment::Inlines(inlines) = parse_fragment("a *b* $x$", false) else {
            panic!("inlines");
        };
        assert_eq!(inlines.len(), 5, "{inlines:?}");
        let Fragment::Blocks(blocks) = parse_fragment("= H\n\ntext", false) else {
            panic!("blocks");
        };
        assert_eq!(blocks.len(), 2);
        let Fragment::Blocks(blocks) = parse_fragment("#set text(red)\nhello", false) else {
            panic!("blocks");
        };
        assert!(matches!(blocks[0].kind, Block::RawTypst(_)), "{blocks:?}");
        let Fragment::Math(nodes) = parse_fragment("x^2 + 1", true) else {
            panic!("math");
        };
        assert_eq!(nodes.len(), 3);
    }

    /// Named arguments of math calls are kept as code (`opts`), written
    /// before the content arguments.
    #[test]
    fn math_calls_keep_named_arguments() {
        let src = "$ lr(size: #55%, (sum_i x_i)) + abs(x/2, size: #2em) + op(\"Ker\", limits: #true)_x + lr(size: #(80% + 2pt), [y]) + cases(delim: \"[\", 1, 2) $";
        let doc = renders_the_same(src);
        let Block::DisplayMath(row) = &doc.blocks[0].kind else {
            panic!("{:?}", doc.blocks[0].kind);
        };
        let opts: Vec<&str> = row
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                model::MathKind::Call { opts, .. } => Some(opts.as_str()),
                model::MathKind::Scripts { base, .. } => match &base.nodes[0].kind {
                    model::MathKind::Call { opts, .. } => Some(opts.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            opts,
            [
                "size: 55%",
                "size: 2em",
                "limits: true",
                "size: (80% + 2pt)",
                "delim: \"[\""
            ]
        );
        assert!(
            serialize_document(&doc).contains("abs(size: #2em, x/2)"),
            "{}",
            serialize_document(&doc)
        );
    }
}
