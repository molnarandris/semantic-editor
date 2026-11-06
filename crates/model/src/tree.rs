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
#[serde(transparent)]
pub struct BlockNode {
    #[serde(skip)]
    pub id: NodeId,
    pub kind: Block,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Heading { level: u8, inlines: Vec<InlineNode> },
    Paragraph(Vec<InlineNode>),
    DisplayMath(MathRow),
    RawTypst(String),
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
}

impl Block {
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
}

impl BlockNode {
    /// The top-level math rows of this block.
    pub fn rows(&self) -> Vec<&MathRow> {
        match &self.kind {
            Block::DisplayMath(row) => vec![row],
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => inlines
                .iter()
                .filter_map(|i| match &i.kind {
                    Inline::Math(row) => Some(row),
                    Inline::Text(_) => None,
                })
                .collect(),
            Block::RawTypst(_) => vec![],
        }
    }

    pub fn rows_mut(&mut self) -> Vec<&mut MathRow> {
        match &mut self.kind {
            Block::DisplayMath(row) => vec![row],
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => inlines
                .iter_mut()
                .filter_map(|i| match &mut i.kind {
                    Inline::Math(row) => Some(row),
                    Inline::Text(_) => None,
                })
                .collect(),
            Block::RawTypst(_) => vec![],
        }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            Block::Heading { .. } => "heading",
            Block::Paragraph(_) => "paragraph",
            Block::DisplayMath(_) => "display math",
            Block::RawTypst(_) => "raw typst",
        }
    }
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
            _ => None,
        }
    }

    /// Child rows in visual/navigation order.
    pub fn children(&self) -> Vec<(Step, &MathRow)> {
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
            _ => vec![],
        }
    }

    /// True for nodes with child rows (fractions, scripts).
    pub fn is_structure(&self) -> bool {
        matches!(self.kind, MathKind::Frac { .. } | MathKind::Scripts { .. })
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            MathKind::Sym(_) => "symbol",
            MathKind::Ident(_) => "identifier",
            MathKind::Num(_) => "number",
            MathKind::Frac { .. } => "fraction",
            MathKind::Scripts { .. } => "scripts",
            MathKind::Slot => "slot",
        }
    }
}

impl MathRow {
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

/// The whole document: a flat list of blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
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
        let mut doc = Self {
            blocks: vec![],
            next_id: 1,
        };
        let p = doc.paragraph(vec![]);
        doc.blocks.push(p);
        doc
    }

    /// A document with exactly the given blocks (built with this document's
    /// builders, or regenerated afterwards).
    pub fn empty() -> Self {
        Self {
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
        let mut next = 1;
        let mut f = |id: &mut NodeId| {
            *id = NodeId(next);
            next += 1;
        };
        for block in &mut self.blocks {
            f(&mut block.id);
            if let Some(inlines) = block.kind.inlines_mut() {
                for inline in inlines.iter_mut() {
                    f(&mut inline.id);
                    if let Inline::Math(row) = &mut inline.kind {
                        row.visit_ids(&mut f);
                    }
                }
            } else if let Block::DisplayMath(row) = &mut block.kind {
                row.visit_ids(&mut f);
            }
        }
        self.next_id = next;
        self.normalize();
    }

    /// Enforce the paragraph invariant: inlines alternate Text, Math, Text, …,
    /// starting and ending with (possibly empty) Text. Keeps a caret position
    /// before and after every inline formula. A document always has a block.
    pub fn normalize(&mut self) {
        let mut blocks = std::mem::take(&mut self.blocks);
        for block in &mut blocks {
            if let Some(inlines) = block.kind.inlines_mut() {
                let old = std::mem::take(inlines);
                *inlines = self.normalized_inlines(old);
            }
        }
        if blocks.is_empty() {
            blocks.push(self.paragraph(vec![]));
        }
        self.blocks = blocks;
    }

    fn normalized_inlines(&mut self, inlines: Vec<InlineNode>) -> Vec<InlineNode> {
        let mut out: Vec<InlineNode> = Vec::with_capacity(inlines.len() + 2);
        for inline in inlines {
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
                (
                    Inline::Math(_),
                    None
                    | Some(InlineNode {
                        kind: Inline::Math(_),
                        ..
                    }),
                ) => {
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
        for block in &self.blocks {
            check(block.id)?;
            if let Some(inlines) = block.kind.inlines() {
                for (i, inline) in inlines.iter().enumerate() {
                    check(inline.id)?;
                    let want_text = i % 2 == 0;
                    match &inline.kind {
                        Inline::Text(_) if want_text => {}
                        Inline::Math(row) if !want_text => row_ids(row, &mut check)?,
                        _ => {
                            return Err(format!(
                                "block {:?}: inline {i} breaks alternation",
                                block.id
                            ));
                        }
                    }
                }
                if inlines.len() % 2 == 0 {
                    return Err(format!(
                        "block {:?}: must start and end with text",
                        block.id
                    ));
                }
            } else if let Block::DisplayMath(row) = &block.kind {
                row_ids(row, &mut check)?;
            }
        }
        Ok(())
    }

    pub fn block(&self, id: NodeId) -> Option<&BlockNode> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn block_index(&self, id: NodeId) -> Option<usize> {
        self.blocks.iter().position(|b| b.id == id)
    }

    pub fn find_row(&self, id: NodeId) -> Option<&MathRow> {
        self.blocks
            .iter()
            .flat_map(|b| b.rows())
            .find_map(|r| r.find_row(id))
    }

    pub fn find_row_mut(&mut self, id: NodeId) -> Option<&mut MathRow> {
        self.blocks
            .iter_mut()
            .flat_map(|b| b.rows_mut())
            .find_map(|r| r.find_row_mut(id))
    }

    // ----- Builders. They allocate fresh ids from this document. -----

    pub fn text(&mut self, s: &str) -> InlineNode {
        InlineNode {
            id: self.fresh_id(),
            kind: Inline::Text(s.into()),
        }
    }

    pub fn math(&mut self, nodes: Vec<MathNode>) -> InlineNode {
        let row = self.row(nodes);
        InlineNode {
            id: self.fresh_id(),
            kind: Inline::Math(row),
        }
    }

    pub fn row(&mut self, nodes: Vec<MathNode>) -> MathRow {
        MathRow {
            id: self.fresh_id(),
            nodes,
        }
    }

    fn node(&mut self, kind: MathKind) -> MathNode {
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
            id: self.fresh_id(),
            kind: Block::Paragraph(inlines),
        }
    }

    pub fn heading(&mut self, level: u8, inlines: Vec<InlineNode>) -> BlockNode {
        let inlines = self.normalized_inlines(inlines);
        BlockNode {
            id: self.fresh_id(),
            kind: Block::Heading { level, inlines },
        }
    }

    pub fn display(&mut self, nodes: Vec<MathNode>) -> BlockNode {
        let row = self.row(nodes);
        BlockNode {
            id: self.fresh_id(),
            kind: Block::DisplayMath(row),
        }
    }

    pub fn raw(&mut self, s: &str) -> BlockNode {
        BlockNode {
            id: self.fresh_id(),
            kind: Block::RawTypst(s.into()),
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

        let json = serde_json::to_string(&doc).unwrap();
        assert!(!json.contains("\"id\""));
        let mut back: Document = serde_json::from_str(&json).unwrap();
        back.regenerate_ids();
        back.check_invariants().unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
}
