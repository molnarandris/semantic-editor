//! Which blocks need a compile: those whose Typst source, span map or
//! surrounding code (header, counter state) changed since it was last
//! submitted. Unchanged blocks, such as diagrams next to edited prose, are
//! never recompiled. A changed map with the same source (node ids replaced,
//! same text) is a cache hit in the worker, but its layout must be resolved
//! against the new ids. No GTK here, so it can be tested directly.
//!
//! Counters are chained through the counter-sensitive blocks (headings,
//! display math, raw Typst): each starts from the counters the previous one
//! ended with. A sensitive block is submitted only once that end state is
//! known, so a change ripples down one compile at a time and stops at the
//! first block whose end state comes out unchanged.

use model::{Block, BlockNode};
use typst_bridge::{
    Around, CounterState, SpanMap, chain_code, initial_state, serialize_block, tracked_counters,
};

/// What was last submitted for a block, and how its counters ended.
pub struct Submitted<'a> {
    pub around: &'a Around,
    pub snippet: &'a str,
    pub map: &'a SpanMap,
    /// Counter-sensitive blocks: the counters at the end of the submitted
    /// render, once it arrived.
    pub end: Option<&'a CounterState>,
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

/// Whether a block may read or change counters.
pub fn counter_sensitive(block: &BlockNode) -> bool {
    matches!(
        block.kind,
        Block::Heading { .. } | Block::DisplayMath(_) | Block::RawTypst(_)
    )
}

/// Jobs for the blocks that differ from `submitted` (the last submitted
/// ones per block, same order). Sensitive blocks wait for the end state of
/// the previous sensitive block.
pub fn stale_blocks(blocks: &[BlockNode], header: &str, submitted: &[Submitted]) -> Vec<Job> {
    let tracked = tracked_counters(header);
    // Counters at the current position; `None` while a block before is
    // still being compiled.
    let mut chain = Some(initial_state(&tracked));
    let mut jobs = vec![];
    for (index, block) in blocks.iter().enumerate() {
        let sensitive = counter_sensitive(block);
        let mut around = Around {
            header: header.into(),
            ..Around::default()
        };
        let start = if sensitive {
            let Some(start) = chain.take() else {
                continue;
            };
            (around.prefix, around.suffix) = chain_code(&tracked, &start);
            Some(start)
        } else {
            None
        };
        let (snippet, map) = serialize_block(block);
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
    }

    fn submitted(views: &[View]) -> Vec<Submitted<'_>> {
        views
            .iter()
            .map(|v| Submitted {
                around: &v.around,
                snippet: &v.snippet,
                map: &v.map,
                end: v.end.as_ref(),
            })
            .collect()
    }

    /// Submit what is stale; returns the indices submitted.
    fn submit(blocks: &[BlockNode], header: &str, views: &mut Vec<View>) -> Vec<usize> {
        views.resize_with(blocks.len(), View::default);
        let jobs = stale_blocks(blocks, header, &submitted(views));
        for job in &jobs {
            views[job.index] = View {
                around: job.around.clone(),
                snippet: job.snippet.clone(),
                map: job.map.clone(),
                end: None,
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
        views[1].end = Some(vec![vec![0], vec![1]]);
        assert!(submit(&d.blocks, "", &mut views).is_empty());
        let snippet = views[0].snippet.clone();
        d.regenerate_ids();
        assert_eq!(serialize_block(&d.blocks[0]).0, snippet);
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
        let state = |h: i64| vec![vec![h], vec![0]];
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
        let state = |h: i64| vec![vec![h], vec![0]];
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
                let jobs = stale_blocks(&d.blocks, header, &submitted(views));
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
        assert_eq!(views[5].end, Some(vec![vec![2], vec![2]]));
        assert_eq!(views[2].end, Some(vec![vec![1, 1], vec![1]]));
        // Again from scratch: all cache hits, same result.
        let mut again = vec![];
        let compiles = worker.compiles();
        settle(&mut again);
        assert_eq!(worker.compiles(), compiles);
        assert_eq!(again[5].end, views[5].end);
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
            let jobs = stale_blocks(blocks, "", &submitted(views));
            for job in &jobs {
                views[job.index] = View {
                    around: job.around.clone(),
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    end: None,
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
}
