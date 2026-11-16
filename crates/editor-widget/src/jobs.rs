//! Which blocks need a compile: those whose Typst source, span map or
//! surrounding code (header, code blocks before it, counter state) changed
//! since it was last submitted. Unchanged blocks, such as diagrams next to
//! edited prose, are never recompiled. A changed map with the same source
//! (node ids replaced, same text) is a cache hit in the worker, but its
//! layout must be resolved against the new ids. No GTK here, so it can be
//! tested directly.
//!
//! Code blocks (raw Typst that only defines things) apply to every block
//! after them: their code is compiled before those blocks, after the header.
//! On the page they show a marker (or their source, with `show_code`).
//!
//! Counters are chained through the counter-sensitive blocks (headings,
//! display math, raw Typst with content): each starts from the counters the
//! previous one ended with. A sensitive block is submitted only once that
//! end state is known, so a change ripples down one compile at a time and
//! stops at the first block whose end state comes out unchanged.

use std::sync::Arc;

use std::collections::HashMap;

use model::{Block, BlockNode, Inline, InlineNode, ListKind, NodeId};
use typst_bridge::bib::{
    bibliography_call, hidden_bibliography, hidden_cites, raw_bibliography_call,
};
use typst_bridge::{
    Around, CounterState, LabelTarget, SpanMap, Style, chain_code, code_display, initial_state,
    is_code, label_report, label_values_in_source, labels_in_source, missing_ref, ref_stub,
    refs_in_source, serialize_block_open, serialize_block_styled, stand_in, tracked_counters,
};

/// What was last submitted for a block, and how its counters ended.
pub struct Submitted<'a> {
    pub around: &'a Around,
    pub snippet: &'a str,
    pub map: &'a SpanMap,
    /// Counter-sensitive blocks: the counters at the end of the submitted
    /// render, once it arrived.
    pub end: Option<&'a CounterState>,
    /// Counter-sensitive blocks: the counters at the start of the
    /// submitted render.
    pub start: Option<&'a CounterState>,
    /// What the labels the block defines point to, as its last render
    /// reported.
    pub targets: &'a [LabelTarget],
}

pub struct Job {
    pub index: usize,
    pub around: Around,
    pub snippet: String,
    pub map: SpanMap,
    /// Slow to compile: keep the render on disk too.
    pub persist: bool,
    /// Counter-sensitive blocks: the counters at the start.
    pub start: Option<CounterState>,
}

/// The labels defined in a block (nested blocks too), each with the Typst
/// code of an invisible element like the one labelled, to stand in for it
/// in blocks that reference it.
fn labels_in(block: &BlockNode, out: &mut Vec<(String, String)>) {
    if !block.label.is_empty() {
        let element = match &block.kind {
            Block::Heading { level, .. } => Some(format!("#heading(level: {level})[]")),
            Block::DisplayMath(_) => Some("#math.equation(block: true)[]".to_string()),
            Block::Env { name, args, .. } if args.is_empty() => Some(format!("#{name}[]")),
            Block::Env { name, args, .. } => Some(format!("#{name}({args})[]")),
            _ => None,
        };
        if let Some(element) = element {
            out.push((block.label.clone(), element));
        }
    }
    // Raw Typst: the stand-in follows from what the label's counter turns
    // out to be (empty here, see `stand_in`).
    if let Block::RawTypst(source) = &block.kind
        && !is_code(source)
    {
        out.extend(
            labels_in_source(source)
                .into_iter()
                .map(|l| (l, String::new())),
        );
    }
    for child in block.kind.body().into_iter().flatten() {
        labels_in(child, out);
    }
}

/// The labels referenced in a block (nested blocks, inline calls and raw
/// Typst too).
fn refs_in(block: &BlockNode, out: &mut Vec<String>) {
    fn inlines(v: &[InlineNode], out: &mut Vec<String>) {
        for inline in v {
            match &inline.kind {
                Inline::Ref(label) if !label.is_empty() && !out.contains(label) => {
                    out.push(label.clone())
                }
                Inline::Call { body, .. } => inlines(body, out),
                _ => {}
            }
        }
    }
    if let Some(v) = block.kind.text_inlines() {
        inlines(v, out);
    }
    // In raw Typst, and in a block call's source arguments
    // (`#proof(title: [Proof of @lem:x])`).
    if let Block::RawTypst(source) | Block::Env { args: source, .. } = &block.kind {
        for label in refs_in_source(source) {
            if !out.contains(&label) {
                out.push(label);
            }
        }
    }
    for child in block.kind.body().into_iter().flatten() {
        refs_in(child, out);
    }
}

/// The labels attached in a block (nested blocks and raw Typst too).
fn all_labels_in(block: &BlockNode, out: &mut Vec<String>) {
    if !block.label.is_empty() {
        out.push(block.label.clone());
    }
    if let Block::RawTypst(source) = &block.kind {
        out.extend(labels_in_source(source));
    }
    for child in block.kind.body().into_iter().flatten() {
        all_labels_in(child, out);
    }
}

/// Problems with labels and references, per top-level block (by index):
/// references to labels that do not exist or that show no number (on raw
/// Typst, unless its block reported a heading, equation or figure; shown
/// red in the editor), and labels defined more than once. Citations are
/// fine: keys of the bibliography block, or of BibTeX entries in the
/// header or raw Typst (a bibliography made from text in the document).
/// `targets`: the labels the compiled blocks reported.
pub fn label_problems(
    header: &str,
    blocks: &[BlockNode],
    targets: &[&LabelTarget],
) -> Vec<(usize, String)> {
    let mut sources = vec![header];
    let mut keys = vec![];
    for block in blocks {
        match &block.kind {
            Block::Bibliography(source) => keys.extend(model::bib_keys(source)),
            Block::RawTypst(source) => sources.push(source),
            _ => {}
        }
    }
    keys.extend(model::embedded_bib_keys(&sources.join("\n")));
    let mut numbered = vec![];
    for block in blocks {
        labels_in(block, &mut numbered);
    }
    let attached: Vec<Vec<String>> = blocks
        .iter()
        .map(|b| {
            let mut v = vec![];
            all_labels_in(b, &mut v);
            v
        })
        .collect();
    let count = |label: &str| attached.iter().flatten().filter(|l| *l == label).count();
    let mut problems = vec![];
    for (index, block) in blocks.iter().enumerate() {
        for label in &attached[index] {
            let n = count(label);
            if n > 1 && attached[..index].iter().flatten().all(|l| l != label) {
                problems.push((index, format!("<{label}> is defined {n} times")));
            }
        }
        let mut refs = vec![];
        refs_in(block, &mut refs);
        for label in refs {
            let has_stand_in = |element: &str| {
                !element.is_empty()
                    || targets
                        .iter()
                        .any(|t| t.label == label && stand_in(t).is_some())
            };
            if numbered.iter().any(|(l, e)| *l == label && has_stand_in(e)) {
                continue;
            }
            if count(&label) > 0 {
                problems.push((
                    index,
                    format!("@{label} points to raw Typst and shows no number"),
                ));
            } else if !keys.contains(&label) {
                problems.push((
                    index,
                    format!("@{label}: no such label or bibliography entry"),
                ));
            }
        }
    }
    problems
}

/// The source of a code block.
fn code_of(block: &BlockNode) -> Option<&str> {
    match &block.kind {
        Block::RawTypst(source) if is_code(source) => Some(source),
        _ => None,
    }
}

/// The blocks (index, counters at their start) labelled by the labels
/// that the raw Typst `block` names as values: each label defined once, by
/// another block that has been rendered.
fn copied_blocks<'a>(
    block: &BlockNode,
    index: usize,
    table: &HashMap<&str, (usize, &str, usize)>,
    submitted: &[Submitted<'a>],
) -> Vec<(usize, &'a CounterState)> {
    let Block::RawTypst(source) = &block.kind else {
        return vec![];
    };
    let mut out: Vec<(usize, &CounterState)> = vec![];
    for label in label_values_in_source(source) {
        if let Some(&(d, _, 1)) = table.get(label.as_str())
            && d != index
            && !out.iter().any(|(e, _)| *e == d)
            && let Some(start) = submitted.get(d).and_then(|s| s.start)
        {
            out.push((d, start));
        }
    }
    out
}

/// Typst code putting a hidden copy of `block` (as exported, label
/// included) where its counters are as at its `start`, so `query` finds
/// it, numbered as in the document. Placed, it takes no space.
fn hidden_copy(block: &BlockNode, start: &CounterState) -> String {
    let (counters, _) = chain_code(start);
    let (source, _) = serialize_block_styled(block, Style::Readable);
    format!("{counters}#place(hide[\n{source}\n])\n")
}

/// The bibliography `block` makes, if any: the call (hidden after citing
/// blocks, so their citations resolve in its style) and its entry keys.
/// The editor's bibliography block, or raw Typst that is one
/// `#bibliography(…)` call with its BibTeX embedded (the user's paper:
/// with a title and a style).
fn bibliography_of(block: &BlockNode) -> Option<(String, Vec<String>)> {
    match &block.kind {
        Block::Bibliography(source) => Some((bibliography_call(source), model::bib_keys(source))),
        Block::RawTypst(source) => Some((
            raw_bibliography_call(source)?,
            model::embedded_bib_keys(source),
        )),
        _ => None,
    }
}

/// Typst code for the full reference that citation `key` points to (shown
/// on hovering the citation), as the bibliography lists it: in its style,
/// numbered as in the document (the document's citations hidden before
/// it). None if `key` is not in the bibliography.
pub fn full_citation(blocks: &[BlockNode], key: &str) -> Option<String> {
    let (call, keys) = blocks.iter().find_map(bibliography_of)?;
    if !keys.iter().any(|k| k == key) {
        return None;
    }
    let mut cited = vec![];
    for block in blocks {
        refs_in(block, &mut cited);
    }
    cited.retain(|r| keys.contains(r));
    Some(format!(
        "{}#cite(label({}), form: \"full\"){}",
        hidden_cites(&cited),
        typst_bridge::serialize::quoted(key),
        hidden_bibliography(&call)
    ))
}

/// Whether a block may read or change counters.
pub fn counter_sensitive(block: &BlockNode) -> bool {
    match &block.kind {
        Block::Heading { .. } | Block::DisplayMath(_) | Block::Env { .. } | Block::Item { .. } => {
            true
        }
        Block::RawTypst(source) => !is_code(source),
        // Footnotes are numbered through the document.
        Block::Paragraph(inlines) => has_footnote(inlines),
        Block::Diagram(_)
        | Block::Caption(_)
        | Block::Bibliography(_)
        | Block::Title(_)
        | Block::Image { .. } => false,
    }
}

/// Whether inlines hold a footnote (in other calls too).
fn has_footnote(inlines: &[InlineNode]) -> bool {
    inlines.iter().any(|i| match &i.kind {
        Inline::Call { name, body, .. } => name == "footnote" || has_footnote(body),
        _ => false,
    })
}

/// Jobs for the blocks that differ from `submitted` (the last submitted
/// ones per block, same order). Sensitive blocks wait for the end state of
/// the previous sensitive block. `show_code` shows code blocks' source
/// instead of a marker.
#[cfg(test)]
pub fn stale_blocks(
    blocks: &[BlockNode],
    header: &str,
    show_code: bool,
    submitted: &[Submitted],
) -> Vec<Job> {
    let files = model::Files::default();
    stale_blocks_editing(
        blocks,
        header,
        &files,
        show_code,
        submitted,
        None,
        Sources::default(),
    )
}

/// Typst code making a paragraph break its lines greedily, as word
/// processors do: an edit then only moves the text from its line on.
/// Typst's optimized breaking (the default for justified text) may move
/// every line of the paragraph at each keystroke, the caret with them.
pub const GREEDY_LINES: &str = "#set par(linebreaks: \"simple\")\n";

/// Nodes whose Typst source shows instead of what it draws.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sources<'a> {
    /// The code piece the caret is in.
    pub open: Option<NodeId>,
    /// Code pieces and calls that failed to compile: on red.
    pub failed: &'a [NodeId],
}

/// The source of node `id` that a compile error is blamed on, if it can
/// show its source when it fails: a code piece, or the name and arguments
/// of a call (inline or a block). A failed one is shown so until this
/// changes.
pub fn failing_source(doc: &model::Document, id: NodeId) -> Option<String> {
    if let Some(block) = doc.block(id) {
        return match &block.kind {
            Block::Env { name, args, .. } => Some(format!("{name}({args})")),
            _ => None,
        };
    }
    match &doc.inline(id)?.kind {
        Inline::Code(s) => Some(s.clone()),
        Inline::Call { name, args, .. } => Some(format!("{name}({args})")),
        _ => None,
    }
}

/// What the document's references and citations point to, for the code
/// around each block that makes them render as in the whole document.
struct Targets<'a> {
    blocks: &'a [BlockNode],
    /// Per block: the labels it defines, each with its stand-in element.
    defined: &'a [Vec<(String, String)>],
    /// By label: the block defining it, its stand-in element and how often
    /// it is defined (a reference resolves only to a unique label).
    table: HashMap<&'a str, (usize, &'a str, usize)>,
    /// What labels point to, as their blocks last reported.
    reported: HashMap<&'a str, &'a LabelTarget>,
    /// The bibliography's block and its call.
    bibliography: Option<(usize, String)>,
    /// Per block: its citations (references to bibliography keys that are
    /// not labels).
    cites: Vec<Vec<String>>,
    /// Every key cited, in document order.
    all_cited: Vec<String>,
}

impl<'a> Targets<'a> {
    fn new(
        blocks: &'a [BlockNode],
        defined: &'a [Vec<(String, String)>],
        submitted: &[Submitted<'a>],
    ) -> Self {
        let mut table: HashMap<&str, (usize, &str, usize)> = HashMap::new();
        for (index, labels) in defined.iter().enumerate() {
            for (label, element) in labels {
                table.entry(label).or_insert((index, element, 0)).2 += 1;
            }
        }
        let mut reported: HashMap<&str, &LabelTarget> = HashMap::new();
        for (index, s) in submitted.iter().enumerate() {
            for t in s.targets {
                if table.get(t.label.as_str()).is_some_and(|d| d.0 == index) {
                    reported.insert(&t.label, t);
                }
            }
        }
        let (bibliography, keys) = blocks
            .iter()
            .enumerate()
            .find_map(|(i, b)| bibliography_of(b).map(|(call, keys)| (Some((i, call)), keys)))
            .unwrap_or_default();
        let cites: Vec<Vec<String>> = blocks
            .iter()
            .map(|b| {
                let mut refs = vec![];
                refs_in(b, &mut refs);
                refs.retain(|r| !table.contains_key(r.as_str()) && keys.contains(r));
                refs
            })
            .collect();
        let mut all_cited: Vec<String> = vec![];
        for key in cites.iter().flatten() {
            if !all_cited.contains(key) {
                all_cited.push(key.clone());
            }
        }
        Self {
            blocks,
            defined,
            table,
            reported,
            bibliography,
            cites,
            all_cited,
        }
    }

    /// The labels block `index` defines.
    fn own(&self, index: usize) -> Vec<String> {
        self.defined[index].iter().map(|(l, _)| l.clone()).collect()
    }

    /// The keys block `index` cites, with those of the blocks `copied`
    /// before it (whose citations are made there too).
    fn cited(&self, index: usize, copied: &[(usize, &CounterState)]) -> Vec<String> {
        let mut cited = self.cites[index].clone();
        for &(d, _) in copied {
            for key in &self.cites[d] {
                if !cited.contains(key) {
                    cited.push(key.clone());
                }
            }
        }
        cited
    }

    /// References of block `index` and of the blocks `copied` before it
    /// (except to labels those define): a stand-in for each unique,
    /// reported target (after everything else), else a red `@label`.
    fn add_references(
        &self,
        index: usize,
        copied: &[(usize, &CounterState)],
        cited: &[String],
        around: &mut Around,
    ) {
        let mut refs = vec![];
        refs_in(&self.blocks[index], &mut refs);
        let mut copy_labels = vec![];
        for &(d, _) in copied {
            refs_in(&self.blocks[d], &mut refs);
            copy_labels.extend(self.own(d));
        }
        refs.retain(|l| !copy_labels.contains(l));
        let own = self.own(index);
        for label in refs
            .iter()
            .filter(|l| !own.contains(l) && !cited.contains(l))
        {
            let stub = match (
                self.table.get(label.as_str()),
                self.reported.get(label.as_str()),
            ) {
                (Some((_, element, 1)), Some(target)) => match *element {
                    "" => stand_in(target),
                    e => Some(e.to_string()),
                }
                .map(|e| ref_stub(target, &e)),
                _ => None,
            };
            match stub {
                Some(stub) => around.suffix.push_str(&stub),
                None => around.prefix.push_str(&missing_ref(label)),
            }
        }
    }

    /// Citations numbered as in the whole document: block `index` citing
    /// `cited` gets those of the blocks before it (`cited_before`, which it
    /// adds to) hidden before it and the bibliography hidden after it; the
    /// bibliography block gets everything the document cites before it.
    fn add_citations(
        &self,
        index: usize,
        cited: &[String],
        cited_before: &mut Vec<String>,
        around: &mut Around,
    ) {
        if let (Some((_, call)), false) = (&self.bibliography, cited.is_empty()) {
            around.prefix.push_str(&hidden_cites(cited_before));
            around.suffix.push_str(&hidden_bibliography(call));
        }
        if self.bibliography.as_ref().is_some_and(|(i, _)| *i == index) {
            around.prefix.push_str(&hidden_cites(&self.all_cited));
        }
        for key in cited {
            if !cited_before.contains(key) {
                cited_before.push(key.clone());
            }
        }
    }
}

/// [`stale_blocks`], with the project's `files`, and `editing` the
/// top-level block being typed in: its lines are broken greedily
/// ([`GREEDY_LINES`]) until the caret leaves it; `sources` the nodes whose
/// source shows.
pub fn stale_blocks_editing(
    blocks: &[BlockNode],
    header: &str,
    files: &model::Files,
    show_code: bool,
    submitted: &[Submitted],
    editing: Option<NodeId>,
    sources: Sources,
) -> Vec<Job> {
    let code: Vec<Option<&str>> = blocks.iter().map(code_of).collect();
    let all_code: String = code
        .iter()
        .flatten()
        .fold(header.to_string(), |all, c| all + c);
    // Counters at the current position; `None` while a block before is
    // still being compiled.
    let mut chain = Some(initial_state(&tracked_counters(&all_code)));
    // Header and code blocks so far.
    let mut context: Arc<str> = header.into();
    let mut jobs = vec![];
    // Place of a numbered item in its list (items are rendered one by one).
    let mut number = 0;
    let defined: Vec<Vec<(String, String)>> = blocks
        .iter()
        .map(|b| {
            let mut v = vec![];
            labels_in(b, &mut v);
            v
        })
        .collect();
    let targets = Targets::new(blocks, &defined, submitted);
    // Cited in the blocks before the current one.
    let mut cited_before: Vec<String> = vec![];
    for (index, block) in blocks.iter().enumerate() {
        number = match block.kind {
            Block::Item {
                kind: ListKind::Numbered,
                ..
            } => number + 1,
            _ => 0,
        };
        let sensitive = code[index].is_none() && counter_sensitive(block);
        let mut around = Around {
            header: context.clone(),
            files: files.clone(),
            ..Around::default()
        };
        if let Some(source) = code[index] {
            around.suffix = code_display(source, show_code);
            context = format!("{context}\n{source}\n").into();
        }
        let start = if sensitive {
            let Some(start) = chain.take() else {
                continue;
            };
            (around.prefix, around.suffix) = chain_code(&start);
            Some(start)
        } else {
            None
        };
        // The block being typed in: greedy lines, so they stay put.
        if editing == Some(block.id)
            && matches!(
                block.kind,
                Block::Paragraph(_) | Block::Env { .. } | Block::Item { .. } | Block::Caption(_)
            )
        {
            around.prefix.push_str(GREEDY_LINES);
        }
        // Blocks labelled by labels named in raw Typst (`#restate(<lem:x>)`,
        // whose function looks them up with `query`): a hidden copy of each
        // first, its counters as at its own start (from its last render);
        // the block's own prefix then sets them back.
        let copied = copied_blocks(block, index, &targets.table, submitted);
        let copies: String = copied
            .iter()
            .map(|&(d, start)| hidden_copy(&blocks[d], start))
            .collect();
        around.prefix.insert_str(0, &copies);
        // Labelled blocks (all counter-sensitive) report their targets.
        if sensitive {
            around.suffix.push_str(&label_report(&targets.own(index)));
        }
        let cited = targets.cited(index, &copied);
        targets.add_references(index, &copied, &cited, &mut around);
        targets.add_citations(index, &cited, &mut cited_before, &mut around);
        let place = (number > 0).then_some(number);
        let (snippet, map) = serialize_block_open(block, place, sources.open, sources.failed);
        let prev = submitted.get(index);
        let current =
            prev.is_some_and(|p| *p.around == around && p.snippet == snippet && *p.map == map);
        if sensitive && current {
            chain = prev.and_then(|p| p.end.cloned());
        }
        if !current {
            jobs.push(Job {
                index,
                around,
                snippet,
                map,
                persist: matches!(block.kind, Block::Diagram(_) | Block::RawTypst(_)),
                start,
            });
        }
    }
    jobs
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{Document, Editor, Step};
    use std::sync::mpsc;
    use std::time::Duration;
    use typst_bridge::{CompileJob, Worker};

    /// A block view as the widget keeps it.
    #[derive(Default)]
    struct View {
        around: Around,
        snippet: String,
        map: SpanMap,
        end: Option<CounterState>,
        start: Option<CounterState>,
        targets: Vec<LabelTarget>,
    }

    fn submitted(views: &[View]) -> Vec<Submitted<'_>> {
        views
            .iter()
            .map(|v| Submitted {
                around: &v.around,
                snippet: &v.snippet,
                map: &v.map,
                end: v.end.as_ref(),
                start: v.start.as_ref(),
                targets: &v.targets,
            })
            .collect()
    }

    /// Heading and equation counters with these values.
    fn counters(heading: &[i64], equation: &[i64]) -> CounterState {
        vec![
            ("counter(heading)".into(), heading.to_vec()),
            ("counter(math.equation)".into(), equation.to_vec()),
            ("counter(footnote)".into(), vec![0]),
        ]
    }

    /// Submit what is stale; returns the indices submitted.
    fn submit(blocks: &[BlockNode], header: &str, views: &mut Vec<View>) -> Vec<usize> {
        views.resize_with(blocks.len(), View::default);
        let jobs = stale_blocks(blocks, header, false, &submitted(views));
        for job in &jobs {
            views[job.index] = View {
                around: job.around.clone(),
                snippet: job.snippet.clone(),
                map: job.map.clone(),
                end: None,
                start: job.start.clone(),
                ..Default::default()
            };
        }
        jobs.iter().map(|j| j.index).collect()
    }

    /// Same text, new node ids: the layout must be resolved again.
    #[test]
    fn new_ids_with_same_source_are_stale() {
        let mut d = Document::empty();
        let x = d.ident("x");
        let t = d.text("x");
        d.blocks = vec![d.paragraph(vec![t]), d.display(vec![x])];
        let mut views = vec![];
        assert_eq!(submit(&d.blocks, "", &mut views), [0, 1]);
        views[1].end = Some(counters(&[0], &[1]));
        assert!(submit(&d.blocks, "", &mut views).is_empty());
        let snippet = views[0].snippet.clone();
        d.regenerate_ids();
        assert_eq!(typst_bridge::serialize_block(&d.blocks[0]).0, snippet);
        assert_eq!(submit(&d.blocks, "", &mut views), [0, 1]);
    }

    /// Sensitive blocks go one after the other; prose does not wait. A
    /// re-render with an unchanged end state stops the ripple; a header
    /// change recompiles everything.
    #[test]
    fn counters_ripple_through_sensitive_blocks_only() {
        let mut d = Document::empty();
        let blocks = vec![
            {
                let t = d.text("One");
                d.heading(1, vec![t])
            },
            {
                let t = d.text("Prose");
                d.paragraph(vec![t])
            },
            {
                let t = d.text("Two");
                d.heading(1, vec![t])
            },
            {
                let x = d.ident("x");
                d.display(vec![x])
            },
            {
                let t = d.text("More prose");
                d.paragraph(vec![t])
            },
        ];
        d.blocks = blocks;
        let state = |h: i64| counters(&[h], &[0]);
        let mut views = vec![];
        assert_eq!(submit(&d.blocks, "", &mut views), [0, 1, 4]);
        assert!(
            submit(&d.blocks, "", &mut views).is_empty(),
            "waits for block 0"
        );
        views[0].end = Some(state(1));
        assert_eq!(submit(&d.blocks, "", &mut views), [2]);
        assert!(views[2].around.prefix.contains("update((1,))"));
        views[2].end = Some(state(2));
        assert_eq!(submit(&d.blocks, "", &mut views), [3]);
        views[3].end = Some(state(2));
        assert!(submit(&d.blocks, "", &mut views).is_empty());

        // Editing the first heading's text: its end state is unchanged.
        let t = d.text("Uno");
        d.blocks[0] = d.heading(1, vec![t]);
        assert_eq!(submit(&d.blocks, "", &mut views), [0]);
        views[0].end = Some(state(1));
        assert!(submit(&d.blocks, "", &mut views).is_empty());

        // A changed end state ripples on.
        let t = d.text("Prose");
        let p = d.paragraph(vec![t]);
        let t = d.text("Zero");
        let h = d.heading(1, vec![t]);
        d.blocks.splice(0..0, [h, p]);
        views.splice(0..0, [View::default(), View::default()]);
        assert_eq!(submit(&d.blocks, "", &mut views), [0, 1]);
        views[0].end = Some(state(1));
        assert_eq!(submit(&d.blocks, "", &mut views), [2]);
        views[2].end = Some(state(2));
        assert_eq!(submit(&d.blocks, "", &mut views), [4]);

        let header = "#set heading(numbering: \"1.\")";
        assert_eq!(submit(&d.blocks, header, &mut views), [0, 1, 3, 6]);
    }

    /// Code blocks apply to the blocks after them and are not part of the
    /// counter chain; editing one recompiles only what follows it, and the
    /// view switch re-renders only code blocks.
    /// Items are rendered one by one, so a numbered item is written with its
    /// place in the list; any other block ends the list.
    #[test]
    fn numbered_items_know_their_place() {
        let mut d = Document::empty();
        let item = |d: &mut Document, s: &str, kind| {
            let t = d.text(s);
            let p = d.paragraph(vec![t]);
            d.item(kind, vec![p])
        };
        let t = d.text("text");
        let blocks = vec![
            item(&mut d, "a", ListKind::Numbered),
            item(&mut d, "b", ListKind::Numbered),
            d.paragraph(vec![t]),
            item(&mut d, "c", ListKind::Numbered),
            item(&mut d, "d", ListKind::Bullet),
        ];
        // Items are counter-sensitive: render them in turn as the chain
        // moves on.
        let mut views: Vec<View> = (0..blocks.len()).map(|_| View::default()).collect();
        for _ in 0..blocks.len() {
            for job in stale_blocks(&blocks, "", false, &submitted(&views)) {
                // As if rendered, with the counters unchanged.
                views[job.index] = View {
                    end: job.start.clone(),
                    around: job.around,
                    snippet: job.snippet,
                    map: job.map,
                    ..Default::default()
                };
            }
        }
        let got: Vec<&str> = views.iter().map(|v| v.snippet.as_str()).collect();
        assert_eq!(got, ["1. a", "2. b", "text", "1. c", "- d"]);
    }

    #[test]
    fn code_blocks_apply_to_later_blocks() {
        let mut d = Document::empty();
        let paragraph = |d: &mut Document, s: &str| {
            let t = d.text(s);
            d.paragraph(vec![t])
        };
        let heading = |d: &mut Document, s: &str| {
            let t = d.text(s);
            d.heading(1, vec![t])
        };
        d.blocks = vec![
            paragraph(&mut d, "Before"),
            d.raw("#let a = 1"),
            paragraph(&mut d, "After"),
            heading(&mut d, "One"),
            d.raw("// appendix\n#set heading(numbering: \"A.1\")"),
            heading(&mut d, "Two"),
        ];
        let mut views = vec![];
        assert_eq!(submit(&d.blocks, "H", &mut views), [0, 1, 2, 3, 4]);
        assert_eq!(&*views[0].around.header, "H");
        assert_eq!(
            &*views[1].around.header, "H",
            "a code block runs after the code before it"
        );
        assert!(views[1].around.suffix.contains("let a"), "marker");
        assert!(views[2].around.header.ends_with("#let a = 1\n"));
        assert!(views[3].around.header.ends_with("#let a = 1\n"));
        views[3].end = Some(counters(&[1], &[0]));
        assert_eq!(submit(&d.blocks, "H", &mut views), [5]);
        assert!(views[5].around.header.contains("numbering: \"A.1\""));
        assert!(views[5].around.prefix.contains("update((1,))"));
        views[5].end = Some(counters(&[2], &[0]));
        assert!(submit(&d.blocks, "H", &mut views).is_empty());

        // Editing the first code block: everything after it, nothing before.
        d.blocks[1] = d.raw("#let a = 2");
        assert_eq!(submit(&d.blocks, "H", &mut views), [1, 2, 3, 4]);
        views[3].end = Some(counters(&[1], &[0]));
        assert_eq!(submit(&d.blocks, "H", &mut views), [5]);
        views[5].end = Some(counters(&[2], &[0]));

        // Showing code: only the code blocks.
        views.resize_with(d.blocks.len(), View::default);
        let jobs = stale_blocks(&d.blocks, "H", true, &submitted(&views));
        let indices: Vec<usize> = jobs.iter().map(|j| j.index).collect();
        assert_eq!(indices, [1, 4]);
        assert!(jobs[0].around.suffix.contains("raw(block: true"));
    }

    /// With the compiler: a definition works in the blocks after its code
    /// block, not before.
    #[test]
    fn definitions_reach_later_blocks_only() {
        let mut d = Document::empty();
        d.blocks = vec![
            d.raw("#greet"),
            d.raw("#let greet = [Hello]"),
            d.raw("#greet"),
        ];
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, move |r| tx.send(r).unwrap());
        let mut views = vec![];
        let mut ok = [None; 3];
        loop {
            views.resize_with(d.blocks.len(), View::default);
            let jobs = stale_blocks(&d.blocks, "", false, &submitted(&views));
            if jobs.is_empty() {
                break;
            }
            for job in &jobs {
                views[job.index] = View {
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    end: None,
                    ..Default::default()
                };
                worker.submit(CompileJob {
                    block: model::NodeId(job.index as u64),
                    generation: 1,
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    width_pt: 300.0,
                    pixel_per_pt: 1.0,
                    persist: false,
                    transient: false,
                });
            }
            for _ in &jobs {
                let r = rx.recv_timeout(Duration::from_secs(120)).unwrap();
                let i = r.block.0 as usize;
                ok[i] = Some(r.outcome.is_ok());
                // As the widget does: a failed block passes its start on.
                let end = r.outcome.ok().and_then(|r| r.end_state);
                views[i].end = end.or_else(|| job_start(&jobs, i));
            }
        }
        assert_eq!(ok, [Some(false), Some(true), Some(true)]);
    }

    /// With the compiler: references render once their targets reported,
    /// also to a later block; a missing label shows red instead of failing.
    /// Also references in raw Typst. Every render compiles, the stand-ins resolve the references.
    #[test]
    fn references_resolve_through_reports() {
        let mut d = Document::empty();
        let (see, r1, and, r2, end) = (
            d.text("See "),
            d.reference("sec:b"),
            d.text(" and "),
            d.reference("nowhere"),
            d.text("."),
        );
        let (also, r3) = (d.text(" Also "), d.reference("fig"));
        let r4 = d.reference("eq:raw");
        let para = d.paragraph(vec![see, r1, and, r2, end, r3, also, r4]);
        let (a, b) = (d.text("A"), d.text("B"));
        let first = d.heading(1, vec![a]);
        let mut second = d.heading(1, vec![b]);
        second.label = "sec:b".into();
        let (body, cap) = (d.text("Body"), d.text("Cap"));
        let (body, cap) = (d.paragraph(vec![body]), d.caption(vec![cap]));
        let mut figure = d.env("figure", vec![body, cap]);
        figure.label = "fig".into();
        let raw = d.raw("#figure([Raw], caption: [See @sec:b, @nowhere.])");
        let equation = d.raw("$ |x| $ <eq:raw>");
        d.blocks = vec![para, first, second, figure, raw, equation];
        let header = "#set heading(numbering: \"1.1\")\n#set math.equation(numbering: \"(1)\")\n";
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, move |r| tx.send(r).unwrap());
        let mut views: Vec<View> = vec![];
        let mut rounds = 0;
        loop {
            views.resize_with(d.blocks.len(), View::default);
            let jobs = stale_blocks(&d.blocks, header, false, &submitted(&views));
            if jobs.is_empty() {
                break;
            }
            rounds += 1;
            assert!(rounds < 10, "settles");
            for job in &jobs {
                views[job.index] = View {
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    end: None,
                    start: job.start.clone(),
                    targets: std::mem::take(&mut views[job.index].targets),
                };
                worker.submit(CompileJob {
                    block: model::NodeId(job.index as u64),
                    generation: 1,
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    width_pt: 300.0,
                    pixel_per_pt: 1.0,
                    persist: false,
                    transient: false,
                });
            }
            for _ in &jobs {
                let r = rx.recv_timeout(Duration::from_secs(120)).unwrap();
                let i = r.block.0 as usize;
                let rendered = r.outcome.unwrap_or_else(|e| panic!("block {i}: {e:?}"));
                views[i].end = rendered.end_state.or_else(|| job_start(&jobs, i));
                views[i].targets = rendered.targets;
            }
        }
        assert_eq!(views[2].targets.len(), 1, "the heading reported its label");
        assert_eq!(views[3].targets.len(), 1, "the figure reported its label");
        assert!(views[0].around.suffix.contains("<sec:b>"), "a stand-in");
        assert!(
            views[0].around.suffix.contains("#figure[] <fig>"),
            "{}",
            views[0].around.suffix
        );
        assert!(views[0].around.prefix.contains("\"@nowhere\""), "red");
        assert!(
            views[4].around.suffix.contains("<sec:b>"),
            "raw: a stand-in"
        );
        assert!(views[4].around.prefix.contains("\"@nowhere\""), "raw: red");
        assert_eq!(
            views[5].targets.len(),
            1,
            "a raw equation reported its label"
        );
        assert!(
            views[0]
                .around
                .suffix
                .contains("#math.equation(block: true)[] <eq:raw>"),
            "a label on raw Typst gets a stand-in: {}",
            views[0].around.suffix
        );
    }

    /// Missing labels, labels only on raw Typst, labels defined twice.
    #[test]
    fn label_problems_are_found() {
        let mut d = Document::empty();
        let (r1, r2, r3) = (
            d.reference("sec:b"),
            d.reference("gone"),
            d.reference("fig:raw"),
        );
        let para = d.paragraph(vec![r1, r2, r3]);
        let (a, b) = (d.text("A"), d.text("B"));
        let mut first = d.heading(1, vec![a]);
        first.label = "sec:b".into();
        let mut second = d.heading(1, vec![b]);
        second.label = "sec:b".into();
        let raw = d.raw("#figure([F], caption: [See @nowhere.]) <fig:raw>");
        d.blocks = vec![para, first, second, raw];
        // Once the raw figure reported its number, the reference shows it.
        let reported = LabelTarget {
            label: "fig:raw".into(),
            counter: "counter(figure.where(kind: image))".into(),
            value: vec![1],
        };
        let problems = label_problems("", &d.blocks, &[&reported]);
        assert!(problems.iter().all(|(_, p)| !p.contains("@fig:raw")));
        assert_eq!(
            label_problems("", &d.blocks, &[]),
            [
                (0, "@gone: no such label or bibliography entry".to_string()),
                (
                    0,
                    "@fig:raw points to raw Typst and shows no number".to_string()
                ),
                (1, "<sec:b> is defined 2 times".to_string()),
                (
                    3,
                    "@nowhere: no such label or bibliography entry".to_string()
                ),
            ]
        );
    }

    fn job_start(jobs: &[Job], index: usize) -> Option<CounterState> {
        jobs.iter().find(|j| j.index == index)?.start.clone()
    }

    /// A forced re-render (zoom, width) clears the submitted snippets; blocks
    /// waiting for the chain are still re-rendered when it reaches them, even
    /// though their counters did not change.
    #[test]
    fn forced_renders_reach_waiting_blocks() {
        let mut d = Document::empty();
        let (a, b, c) = (d.text("A"), d.text("Prose"), d.text("B"));
        d.blocks = vec![
            d.heading(1, vec![a]),
            d.paragraph(vec![b]),
            d.heading(1, vec![c]),
        ];
        let state = |h: i64| counters(&[h], &[0]);
        let mut views = vec![];
        assert_eq!(submit(&d.blocks, "", &mut views), [0, 1]);
        views[0].end = Some(state(1));
        assert_eq!(submit(&d.blocks, "", &mut views), [2]);
        views[2].end = Some(state(2));

        for view in &mut views {
            view.snippet.clear();
        }
        assert_eq!(submit(&d.blocks, "", &mut views), [0, 1]);
        views[0].end = Some(state(1));
        assert_eq!(submit(&d.blocks, "", &mut views), [2]);
    }

    /// With the compiler: citations render (each block compiles), numbered
    /// through the document; the bibliography gets them all.
    #[test]
    fn citations_through_the_document() {
        let mut e = Editor::new(Document::new());
        e.insert_text("First @beta. ");
        e.enter();
        e.insert_text("Then @alpha and @beta, not @gamma. ");
        e.insert_block(model::NewBlock::Bibliography);
        let mut d = e.doc().clone();
        let bib = "@article{alpha, title = {A}, author = {Ann}, year = {2020}}\n\
                   @article{beta, title = {B}, author = {Bob}, year = {2021}}";
        if let Block::Bibliography(source) = &mut d.blocks[2].kind {
            *source = bib.into();
        }
        let jobs = stale_blocks(&d.blocks, "", false, &[]);
        let around = |i: usize| &jobs.iter().find(|j| j.index == i).unwrap().around;
        let cite = |k: &str| format!("#cite(label(\"{k}\"))");
        assert!(around(0).suffix.contains("#place(hide(bibliography("));
        assert!(!around(0).prefix.contains("#cite"), "nothing cited before");
        assert!(around(1).prefix.contains(&cite("beta")));
        assert!(around(1).prefix.contains("\"@gamma\""), "not a key: red");
        let all = format!("#place(hide[{}{}])", cite("beta"), cite("alpha"));
        assert!(around(2).prefix.contains(&all), "{}", around(2).prefix);
        let mut world = typst_bridge::BridgeWorld::new();
        for job in &jobs {
            let (src, _) = typst_bridge::block_document(&job.around, &job.snippet, 300.0);
            typst_bridge::compile(&mut world, &src)
                .unwrap_or_else(|e| panic!("block {}: {e:?}\n{src}", job.index));
        }

        // The same with the bibliography written by hand as raw Typst, with
        // a title and a style (the user's paper): citations get its call,
        // and it gets all the citations, so it lists them.
        d.blocks[2] = d.raw(&format!(
            "#bibliography(bytes(```\n{bib}\n```.text), title: \"References\", style: \"ieee\")"
        ));
        assert_eq!(d.bib_keys(), ["alpha", "beta"]);
        let jobs = stale_blocks(&d.blocks, "", false, &[]);
        let around = |i: usize| &jobs.iter().find(|j| j.index == i).unwrap().around;
        assert!(
            around(0)
                .suffix
                .contains("title: \"References\", style: \"ieee\")))")
        );
        assert!(around(1).prefix.contains(&cite("beta")));
        assert!(around(2).prefix.contains(&all), "{}", around(2).prefix);
        for job in &jobs {
            let (src, _) = typst_bridge::block_document(&job.around, &job.snippet, 300.0);
            typst_bridge::compile(&mut world, &src)
                .unwrap_or_else(|e| panic!("block {}: {e:?}\n{src}", job.index));
        }
    }

    /// With the compiler: footnotes are numbered through the document (a
    /// paragraph with one is counter-sensitive; one without is not).
    #[test]
    fn footnotes_are_numbered_through_the_chain() {
        let mut e = Editor::new(Document::new());
        e.insert_text("One#footnote[First.] a.");
        e.enter();
        e.insert_text("No note.");
        e.enter();
        e.insert_text("Two *b#footnote[Second.]*.");
        let d = e.doc().clone();
        assert_eq!(
            d.blocks.iter().map(counter_sensitive).collect::<Vec<_>>(),
            [true, false, true]
        );
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, move |r| tx.send(r).unwrap());
        let mut views: Vec<View> = vec![];
        for _ in 0..10 {
            views.resize_with(d.blocks.len(), View::default);
            let jobs = stale_blocks(&d.blocks, "", false, &submitted(&views));
            if jobs.is_empty() {
                break;
            }
            for job in &jobs {
                views[job.index] = View {
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    ..Default::default()
                };
                worker.submit(CompileJob {
                    block: model::NodeId(job.index as u64),
                    generation: 1,
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    width_pt: 300.0,
                    pixel_per_pt: 1.0,
                    persist: false,
                    transient: false,
                });
            }
            for _ in &jobs {
                let r = rx.recv_timeout(Duration::from_secs(120)).unwrap();
                let i = r.block.0 as usize;
                let rendered = r.outcome.unwrap_or_else(|e| panic!("block {i}: {e:?}"));
                views[i].end = rendered.end_state;
            }
        }
        let footnotes = |v: &View| {
            let end = v.end.as_ref()?;
            end.iter()
                .find(|(c, _)| c == "counter(footnote)")
                .map(|(_, n)| n.clone())
        };
        assert_eq!(footnotes(&views[0]), Some(vec![1]));
        assert_eq!(footnotes(&views[2]), Some(vec![2]), "numbered on");
        assert!(
            views[2]
                .around
                .prefix
                .contains("counter(footnote).update((1,))")
        );
    }

    /// With the real worker: submitting until nothing is stale ends with
    /// the right counters, both compiled and from the cache.
    #[test]
    fn chain_settles_with_the_worker() {
        let mut d = Document::empty();
        let heading = |d: &mut Document, level, s: &str| {
            let t = d.text(s);
            d.heading(level, vec![t])
        };
        let blocks = vec![
            heading(&mut d, 1, "A"),
            heading(&mut d, 2, "B"),
            {
                let x = d.ident("x");
                d.display(vec![x])
            },
            {
                let t = d.text("Prose");
                d.paragraph(vec![t])
            },
            heading(&mut d, 1, "C"),
            {
                let y = d.ident("y");
                d.display(vec![y])
            },
        ];
        d.blocks = blocks;
        let header = "#set heading(numbering: \"1.1\")\n#set math.equation(numbering: \"(1)\")";

        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, move |r| tx.send(r).unwrap());
        let settle = |views: &mut Vec<View>| {
            let mut rounds = 0;
            loop {
                views.resize_with(d.blocks.len(), View::default);
                let jobs = stale_blocks(&d.blocks, header, false, &submitted(views));
                if jobs.is_empty() {
                    return rounds;
                }
                rounds += 1;
                for job in &jobs {
                    views[job.index] = View {
                        around: job.around.clone(),
                        snippet: job.snippet.clone(),
                        map: job.map.clone(),
                        end: None,
                        ..Default::default()
                    };
                    worker.submit(CompileJob {
                        block: model::NodeId(job.index as u64),
                        generation: 1,
                        around: job.around.clone(),
                        snippet: job.snippet.clone(),
                        map: job.map.clone(),
                        width_pt: 300.0,
                        pixel_per_pt: 1.0,
                        persist: false,
                        transient: false,
                    });
                }
                for _ in &jobs {
                    let r = rx.recv_timeout(Duration::from_secs(120)).unwrap();
                    let rendered = r.outcome.unwrap();
                    let view = &mut views[r.block.0 as usize];
                    if counter_sensitive(&d.blocks[r.block.0 as usize]) {
                        view.end = rendered.end_state;
                    }
                }
            }
        };
        let mut views = vec![];
        // One round per sensitive block (the first takes the prose along).
        assert_eq!(settle(&mut views), 5);
        assert_eq!(views[5].end, Some(counters(&[2], &[2])));
        assert_eq!(views[2].end, Some(counters(&[1, 1], &[1])));
        // Again from scratch: all cache hits, same result.
        let mut again = vec![];
        let compiles = worker.compiles();
        settle(&mut again);
        assert_eq!(worker.compiles(), compiles);
        assert_eq!(again[5].end, views[5].end);
    }

    /// A reference in a block call's arguments is seen like one in its
    /// text (the user's paper: `#proof(title: [Proof of @lem:main_mps])`
    /// failed with "label does not exist").
    #[test]
    fn references_in_call_arguments() {
        let mut d = Document::empty();
        let t = d.text("Body.");
        let p = d.paragraph(vec![t]);
        let mut proof = d.env("proof", vec![p]);
        if let Block::Env { args, .. } = &mut proof.kind {
            *args = "title: [Proof of @lem:a]".into();
        }
        d.blocks = vec![proof];
        let mut refs = vec![];
        refs_in(&d.blocks[0], &mut refs);
        assert_eq!(refs, ["lem:a"]);
        let jobs = stale_blocks(
            &d.blocks,
            "#let proof(title: none, body) = body",
            false,
            &[],
        );
        assert!(
            jobs[0].around.prefix.contains("\"@lem:a\""),
            "red, compiles"
        );
    }

    /// The block being typed in breaks its lines greedily, the others as
    /// the document says; when typing ends there, it renders again.
    #[test]
    fn the_edited_block_breaks_lines_greedily() {
        let mut e = Editor::new(Document::new());
        e.insert_text("First.");
        e.enter();
        e.insert_text("Second.");
        let d = e.doc().clone();
        let (first, second) = (d.blocks[0].id, d.blocks[1].id);
        let jobs = stale_blocks_editing(
            &d.blocks,
            "",
            &d.files,
            false,
            &[],
            Some(second),
            Sources::default(),
        );
        let around = |i: usize| &jobs.iter().find(|j| j.index == i).unwrap().around;
        assert!(!around(0).prefix.contains(GREEDY_LINES));
        assert!(around(1).prefix.contains(GREEDY_LINES));
        let mut views = vec![];
        views.resize_with(2, View::default);
        for job in &jobs {
            views[job.index].around = job.around.clone();
            views[job.index].snippet = job.snippet.clone();
            views[job.index].map = job.map.clone();
        }
        let again = stale_blocks_editing(
            &d.blocks,
            "",
            &d.files,
            false,
            &submitted(&views),
            Some(first),
            Sources::default(),
        );
        let indices: Vec<usize> = again.iter().map(|j| j.index).collect();
        assert_eq!(indices, [0, 1], "both change how they break");
    }

    /// Measured: in justified text, Typst's default (optimized) breaking
    /// can move lines above an edit; greedy breaking never does. Words
    /// typed into the last line leave the lines above it as they were.
    #[test]
    fn greedy_lines_above_an_edit_stay() {
        let words = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do \
                     eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim \
                     ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut \
                     aliquip ex ea commodo consequat. Duis aute irure dolor in";
        let render = |rule: &str, text: &str| {
            let src = format!(
                "#set page(width: 12cm, height: auto, margin: 0pt)\n\
                 #set par(justify: true)\n{rule}{text}"
            );
            let mut world = typst_bridge::BridgeWorld::new();
            let doc = typst_bridge::compile(&mut world, &src).unwrap();
            typst_bridge::render_page(&doc.pages()[0], 2.0)
        };
        // The top half of the paragraph, well above its last line.
        let top = |original: &typst_bridge::Raster, r: &typst_bridge::Raster| {
            let rows = original.height as usize / 2;
            r.data[..rows * r.width as usize * 4].to_vec()
        };
        let mut moved_optimized = false;
        for extra in [
            "x",
            " longer",
            " a much longer word",
            " nisi ut aliquip ex ea",
        ] {
            let edited = format!("{words}{extra}");
            let (a, b) = (render(GREEDY_LINES, words), render(GREEDY_LINES, &edited));
            assert!(top(&a, &a) == top(&a, &b), "greedy moved lines, {extra:?}");
            let (c, d) = (render("", words), render("", &edited));
            moved_optimized |= top(&c, &c) != top(&c, &d);
        }
        println!("optimized moved lines above the edit: {moved_optimized}");
    }

    /// A raw block naming a label (`#restate(<lem:a>)`, whose function
    /// looks the lemma up with `query`) compiles with a hidden copy of the
    /// lemma before it, numbered as in the document: the restate sees the
    /// lemma's number and the equation count at the lemma (user report:
    /// "array is empty"). `check` records what it saw.
    #[test]
    fn restated_blocks_find_what_they_name() {
        let header = r#"#set math.equation(numbering: "(1)")
#let check = counter("check")
#let lemma(body) = figure(kind: "lemma", supplement: [Lemma], numbering: "1", body)
#let restate(lbl) = context {
  let f = query(lbl).first()
  let eq = counter(math.equation)
  let start = eq.get().first()
  check.update((f.counter.at(lbl).first(), start - eq.at(f.location()).first()))
  f.body
  eq.update(start)
}"#;
        let mut d = Document::empty();
        let display = |d: &mut Document, s: &str| {
            let x = d.ident(s);
            d.display(vec![x])
        };
        let mut blocks = vec![display(&mut d, "x")];
        let inner = display(&mut d, "y");
        let mut lemma = d.env("lemma", vec![inner]);
        lemma.label = "lem:a".into();
        blocks.push(lemma);
        blocks.push(display(&mut d, "z"));
        blocks.push(display(&mut d, "w"));
        blocks.push(d.raw("#restate(<lem:a>)"));
        d.blocks = blocks;

        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, move |r| tx.send(r).unwrap());
        let mut views: Vec<View> = vec![];
        for _ in 0..10 {
            views.resize_with(d.blocks.len(), View::default);
            let jobs = stale_blocks(&d.blocks, header, false, &submitted(&views));
            if jobs.is_empty() {
                break;
            }
            for job in &jobs {
                views[job.index] = View {
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    start: job.start.clone(),
                    ..Default::default()
                };
                worker.submit(CompileJob {
                    block: model::NodeId(job.index as u64),
                    generation: 1,
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    width_pt: 300.0,
                    pixel_per_pt: 1.0,
                    persist: false,
                    transient: false,
                });
            }
            for _ in &jobs {
                let r = rx.recv_timeout(Duration::from_secs(120)).unwrap();
                let i = r.block.0 as usize;
                let rendered = r.outcome.unwrap_or_else(|e| panic!("block {i}: {e:?}"));
                views[i].end = rendered.end_state;
                views[i].targets = rendered.targets;
            }
        }
        let end = views[4].end.as_ref().expect("the restate compiled");
        let check = end.iter().find(|(c, _)| c == "counter(\"check\")");
        // Lemma 1; equations: x (1), in the lemma y (2), z (3), w (4).
        assert_eq!(check.map(|(_, v)| v.clone()), Some(vec![1, 3]));
        let eq = end.iter().find(|(c, _)| c == "counter(math.equation)");
        assert_eq!(eq.map(|(_, v)| v.clone()), Some(vec![4]), "set back");
        assert!(views[4].around.prefix.contains("#place(hide["));
    }

    /// The milestone's acceptance check: editing prose next to 20 diagrams
    /// compiles only the prose.
    #[test]
    fn prose_edits_never_recompile_diagrams() {
        let mut d = Document::empty();
        let t = d.text("Prose");
        let mut blocks = vec![d.paragraph(vec![t])];
        for k in 0..20 {
            blocks.push(d.diagram(&format!("circle((0, 0), radius: {})", 1 + k % 3)));
        }
        d.blocks = blocks;
        let mut editor = Editor::new(d);

        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, move |r| tx.send(r).unwrap());
        let mut views = vec![];
        let submit = |editor: &Editor, views: &mut Vec<View>| {
            let blocks = &editor.doc().blocks;
            views.resize_with(blocks.len(), View::default);
            let jobs = stale_blocks(blocks, "", false, &submitted(views));
            for job in &jobs {
                views[job.index] = View {
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    end: None,
                    ..Default::default()
                };
                worker.submit(CompileJob {
                    block: blocks[job.index].id,
                    generation: 1,
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    width_pt: 300.0,
                    pixel_per_pt: 1.0,
                    persist: job.persist,
                    transient: false,
                });
            }
            for _ in &jobs {
                let r = rx.recv_timeout(Duration::from_secs(120)).unwrap();
                assert!(r.outcome.is_ok(), "{:?}", r.outcome.err());
            }
            jobs.len()
        };

        assert_eq!(submit(&editor, &mut views), 21);
        let initial = worker.compiles();
        let block = editor.doc().blocks[0].id;
        editor.set_caret(model::CursorPath {
            block,
            path: vec![Step::Index(0)],
            offset: 5,
        });
        for word in [" and", " more", " text"] {
            editor.insert_text(word);
            assert_eq!(submit(&editor, &mut views), 1, "only the paragraph");
        }
        editor.enter();
        editor.insert_text("New paragraph");
        submit(&editor, &mut views);
        // Three edits plus the new paragraph (Enter at the end leaves the
        // first paragraph's source unchanged): no diagram at all.
        assert_eq!(worker.compiles() - initial, 4);
    }

    /// Hovering a citation: its full reference, numbered as in the
    /// document; nothing for a key the bibliography lacks.
    #[test]
    fn full_citations() {
        let bib = "@article{alpha, title = {First Paper}, author = {Alpha, Ann}, \
                   journal = {J. Tests}, year = {2020}}\n\
                   @article{beta, title = {Second Paper}, author = {Beta, Bob}, \
                   journal = {J. Tests}, year = {2021}}";
        let doc = typst_bridge::import_typ(&format!(
            "First @beta.\n\nThen @alpha.\n\n#bibliography(bytes(```bib\n{bib}\n```.text))\n"
        ));
        assert_eq!(full_citation(&doc.blocks, "gamma"), None);
        let code = full_citation(&doc.blocks, "alpha").unwrap();
        let (src, _) = typst_bridge::block_document(&Around::default(), &code, 300.0);
        let mut world = typst_bridge::BridgeWorld::new();
        let out = typst_bridge::compile(&mut world, &src).unwrap_or_else(|e| panic!("{e:?}"));
        fn walk(frame: &typst::layout::Frame, out: &mut String) {
            for (_, item) in frame.items() {
                match item {
                    typst::layout::FrameItem::Group(g) => walk(&g.frame, out),
                    typst::layout::FrameItem::Text(t) => out.push_str(&t.text),
                    _ => {}
                }
            }
        }
        let mut text = String::new();
        walk(&out.pages()[0].frame, &mut text);
        assert!(text.starts_with("[2]"), "{text:?}");
        assert!(
            text.contains("First Paper") && !text.contains("Second"),
            "{text:?}"
        );
    }
}
