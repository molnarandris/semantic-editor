//! The document tree. Every node carries a stable [`NodeId`] that is never
//! reused within a document. Ids are not serialized; they are regenerated on
//! load.

use serde::{Deserialize, Serialize};

use crate::cursor::Step;

#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct NodeId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockNode {
    #[serde(skip)]
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
            Inline::Call { name, .. } => match name.as_str() {
                "strong" => "bold",
                "emph" => "italic",
                "link" => "link",
                _ => "function",
            },
        }
    }
}

/// The formulas of an inline list, in calls too.
fn inline_rows<'a>(inlines: &'a [InlineNode], out: &mut Vec<&'a MathRow>) {
    for inline in inlines {
        match &inline.kind {
            Inline::Math(row) => out.push(row),
            Inline::Call { body, .. } => inline_rows(body, out),
            Inline::Text(_) | Inline::Raw(_) => {}
        }
    }
}

fn inline_rows_mut<'a>(inlines: &'a mut [InlineNode], out: &mut Vec<&'a mut MathRow>) {
    for inline in inlines {
        match &mut inline.kind {
            Inline::Math(row) => out.push(row),
            Inline::Call { body, .. } => inline_rows_mut(body, out),
            Inline::Text(_) | Inline::Raw(_) => {}
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MathNode {
    #[serde(skip)]
    pub id: NodeId,
    pub kind: MathKind,
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
    /// it, the caret moves through the arguments.
    Call {
        name: String,
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

    /// The inlines the caret can type in: a paragraph's or heading's, an
    /// environment's title or a term item's term.
    pub fn text_inlines(&self) -> Option<&Vec<InlineNode>> {
        match self {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => Some(inlines),
            Block::Env { title, .. } | Block::Item { term: title, .. } => Some(title),
            _ => None,
        }
    }

    pub fn text_inlines_mut(&mut self) -> Option<&mut Vec<InlineNode>> {
        match self {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => Some(inlines),
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
            | Block::Env { title: inlines, .. }
            | Block::Item { term: inlines, .. } => {
                let mut out = vec![];
                inline_rows(inlines, &mut out);
                out
            }
            Block::RawTypst(_) | Block::Diagram(_) => vec![],
        }
    }

    pub fn rows_mut(&mut self) -> Vec<&mut MathRow> {
        match &mut self.kind {
            Block::DisplayMath(row) => vec![row],
            Block::Paragraph(inlines)
            | Block::Heading { inlines, .. }
            | Block::Env { title: inlines, .. }
            | Block::Item { term: inlines, .. } => {
                let mut out = vec![];
                inline_rows_mut(inlines, &mut out);
                out
            }
            Block::RawTypst(_) | Block::Diagram(_) => vec![],
        }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            Block::Heading { .. } => "heading",
            Block::Paragraph(_) => "paragraph",
            Block::DisplayMath(_) => "display math",
            Block::RawTypst(_) => "raw typst",
            Block::Diagram(_) => "diagram",
            Block::Env { .. } => "environment",
            Block::Item { .. } => "list item",
        }
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

/// The whole document: a list of blocks; environments hold blocks of their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Typst code compiled before every block and at the top of the export:
    /// set/show rules, imports, definitions. Should not produce content.
    pub header: String,
    pub blocks: Vec<BlockNode>,
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
            next_id: 1,
        }
    }

    pub fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Assign new ids to every node, e.g. after deserialization, and restore
    /// the structural invariants.
    pub fn regenerate_ids(&mut self) {
        fn inline_ids(inlines: &mut [InlineNode], f: &mut impl FnMut(&mut NodeId)) {
            for inline in inlines {
                f(&mut inline.id);
                match &mut inline.kind {
                    Inline::Math(row) => row.visit_ids(f),
                    Inline::Call { body, .. } => inline_ids(body, f),
                    Inline::Text(_) | Inline::Raw(_) => {}
                }
            }
        }
        fn visit(blocks: &mut [BlockNode], f: &mut impl FnMut(&mut NodeId)) {
            for block in blocks {
                f(&mut block.id);
                match &mut block.kind {
                    Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
                        inline_ids(inlines, f);
                    }
                    Block::DisplayMath(row) => row.visit_ids(f),
                    Block::Env { title, body, .. }
                    | Block::Item {
                        term: title, body, ..
                    } => {
                        inline_ids(title, f);
                        visit(body, f);
                    }
                    Block::RawTypst(_) | Block::Diagram(_) => {}
                }
            }
        }
        let mut next = 1;
        visit(&mut self.blocks, &mut |id: &mut NodeId| {
            *id = NodeId(next);
            next += 1;
        });
        self.next_id = next;
        self.normalize();
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
            if let Some(inlines) = block.kind.inlines_mut() {
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
    pub(crate) fn normalized_inlines(&mut self, inlines: Vec<InlineNode>) -> Vec<InlineNode> {
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
                (Inline::Math(_) | Inline::Call { .. } | Inline::Raw(_), last)
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
                    Inline::Raw(_) if !want_text => {}
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
        self.node(MathKind::Call {
            name: name.into(),
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

    pub fn diagram(&mut self, source: &str) -> BlockNode {
        BlockNode {
            label: String::new(),
            id: self.fresh_id(),
            kind: Block::Diagram(source.into()),
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
    fn json_roundtrip_regenerates_ids() {
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
        let json = serde_json::to_string(&doc).unwrap();
        assert!(!json.contains("\"id\""));
        // Blocks without a label are written as before labels existed.
        assert!(
            json.contains("{\"Paragraph\":[{\"Text\":\"see \"}"),
            "{json}"
        );
        assert!(json.contains("\"label\":\"eq:frac\""), "{json}");
        let mut back: Document = serde_json::from_str(&json).unwrap();
        back.regenerate_ids();
        back.check_invariants().unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
}
