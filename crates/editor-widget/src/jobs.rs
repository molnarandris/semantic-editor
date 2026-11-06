//! Which blocks need a compile: those whose Typst source or span map changed
//! since it was last submitted. Unchanged blocks, such as diagrams next to
//! edited prose, are never recompiled. A changed map with the same source
//! (node ids replaced, same text) is a cache hit in the worker, but its
//! layout must be resolved against the new ids. No GTK here, so it can be
//! tested directly.

use model::{Block, BlockNode};
use typst_bridge::{SpanMap, serialize_block};

pub struct Job {
    pub index: usize,
    pub snippet: String,
    pub map: SpanMap,
    /// Slow to compile: keep the render on disk too.
    pub persist: bool,
}

/// Jobs for the blocks whose snippet or map differs from `submitted` (the
/// last submitted ones per block, same order), or all with `force`.
pub fn stale_blocks(blocks: &[BlockNode], submitted: &[(&str, &SpanMap)], force: bool) -> Vec<Job> {
    blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            let (snippet, map) = serialize_block(block);
            if !force && submitted.get(index) == Some(&(snippet.as_str(), &map)) {
                return None;
            }
            let persist = matches!(block.kind, Block::Diagram(_) | Block::RawTypst(_));
            Some(Job {
                index,
                snippet,
                map,
                persist,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{Document, Editor, Step};
    use std::sync::mpsc;
    use std::time::Duration;
    use typst_bridge::{CompileJob, Worker};

    /// Same text, new node ids: the layout must be resolved again.
    #[test]
    fn new_ids_with_same_source_are_stale() {
        let mut d = Document::empty();
        let x = d.ident("x");
        d.blocks = vec![d.display(vec![x])];
        let (snippet, map) = serialize_block(&d.blocks[0]);
        let same = [(snippet.as_str(), &map)];
        assert!(stale_blocks(&d.blocks, &same, false).is_empty());
        d.regenerate_ids();
        assert_eq!(serialize_block(&d.blocks[0]).0, snippet);
        assert_eq!(stale_blocks(&d.blocks, &same, false).len(), 1);
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
        let mut submitted: Vec<(String, SpanMap)> = vec![];
        let submit = |editor: &Editor, submitted: &mut Vec<(String, SpanMap)>| {
            let shown: Vec<_> = submitted.iter().map(|(s, m)| (s.as_str(), m)).collect();
            let jobs = stale_blocks(&editor.doc().blocks, &shown, false);
            *submitted = editor.doc().blocks.iter().map(serialize_block).collect();
            for job in &jobs {
                worker.submit(CompileJob {
                    block: editor.doc().blocks[job.index].id,
                    generation: 1,
                    snippet: job.snippet.clone(),
                    map: job.map.clone(),
                    width_pt: 300.0,
                    pixel_per_pt: 1.0,
                    persist: job.persist,
                });
            }
            for _ in &jobs {
                let r = rx.recv_timeout(Duration::from_secs(120)).unwrap();
                assert!(r.outcome.is_ok(), "{:?}", r.outcome.err());
            }
            jobs.len()
        };

        assert_eq!(submit(&editor, &mut submitted), 21);
        let initial = worker.compiles();
        let block = editor.doc().blocks[0].id;
        editor.set_caret(model::CursorPath {
            block,
            path: vec![Step::Index(0)],
            offset: 5,
        });
        for word in [" and", " more", " text"] {
            editor.insert_text(word);
            assert_eq!(submit(&editor, &mut submitted), 1, "only the paragraph");
        }
        editor.enter();
        editor.insert_text("New paragraph");
        submit(&editor, &mut submitted);
        // Three edits plus the new paragraph (Enter at the end leaves the
        // first paragraph's source unchanged): no diagram at all.
        assert_eq!(worker.compiles() - initial, 4);
    }
}
