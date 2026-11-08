//! Counter state chained through blocks.
//!
//! Every block compiles as its own document, so Typst counters (heading
//! numbers, equation numbers, the header's own counters) would restart in
//! each one. A counter-sensitive block therefore gets a prefix that sets the
//! counters to their values at its start, and a suffix that reports their
//! values at its end as labelled metadata, which the next sensitive block
//! starts from.

use typst::foundations::{Label, Value};
use typst::introspection::{Introspector, MetadataElem};
use typst::utils::PicoStr;
use typst_layout::PagedDocument;

/// Values of the tracked counters, in the order of [`tracked_counters`].
pub type CounterState = Vec<Vec<i64>>;

/// Label of the metadata that reports a block's end state.
const END_LABEL: &str = "sem-end";

/// The counters to carry from block to block, as Typst expressions:
/// headings, equations, and every `counter("…")` the header names.
pub fn tracked_counters(header: &str) -> Vec<String> {
    let mut tracked = vec![
        "counter(heading)".to_string(),
        "counter(math.equation)".to_string(),
    ];
    let mut rest = header;
    while let Some(start) = rest.find("counter(\"") {
        let name_start = start + "counter(\"".len();
        let Some(len) = rest[name_start..].find('"') else {
            break;
        };
        let expr = format!("counter(\"{}\")", &rest[name_start..name_start + len]);
        if !tracked.contains(&expr) {
            tracked.push(expr);
        }
        rest = &rest[name_start + len..];
    }
    tracked
}

/// The counters at the start of a document.
pub fn initial_state(tracked: &[String]) -> CounterState {
    vec![vec![0]; tracked.len()]
}

/// Typst code around a sensitive block's snippet: the prefix sets the
/// tracked counters to `start`, the suffix reports their values at the end.
pub fn chain_code(tracked: &[String], start: &CounterState) -> (String, String) {
    let mut prefix = String::new();
    for (expr, value) in tracked.iter().zip(start) {
        let items: Vec<String> = value.iter().map(i64::to_string).collect();
        // `(3,)` is a one-element array, `(3)` just a number.
        let comma = if items.len() == 1 { "," } else { "" };
        prefix.push_str(&format!("#{expr}.update(({}{comma}))\n", items.join(", ")));
    }
    let gets: Vec<String> = tracked.iter().map(|e| format!("{e}.get()")).collect();
    let suffix = format!(
        "\n#context [#metadata(({},)) <{END_LABEL}>]",
        gets.join(", ")
    );
    (prefix, suffix)
}

/// The end state a compiled block reported, if it has a chain suffix.
pub fn end_state(doc: &PagedDocument) -> Option<CounterState> {
    let label = Label::new(PicoStr::intern(END_LABEL))?;
    let content = doc.introspector().query_label(label).ok()?;
    let Value::Array(counters) = &content.to_packed::<MetadataElem>()?.value else {
        return None;
    };
    counters
        .iter()
        .map(|counter| match counter {
            Value::Array(levels) => levels
                .iter()
                .map(|v| match v {
                    Value::Int(n) => Some(*n),
                    _ => None,
                })
                .collect(),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Around, BridgeWorld, block_document, compile};

    const HEADER: &str = "\
#set heading(numbering: \"1.1\")
#set math.equation(numbering: \"(1)\")
#let thm-count = counter(\"thm\")
";

    /// Counter-sensitive blocks of a small document.
    const BLOCKS: &[&str] = &[
        "= Intro",
        "$ a = b $",
        "== Details",
        "#counter(\"thm\").step()\n*Theorem #context counter(\"thm\").display().*",
        "$ c = d $",
        "= Next",
        "== More",
        "#counter(\"thm\").step()\n*Theorem #context counter(\"thm\").display().*",
        "$ e = f $",
    ];

    fn compile_block(world: &mut BridgeWorld, around: &Around, snippet: &str) -> PagedDocument {
        let (src, _) = block_document(around, snippet, 300.0);
        compile(world, &src).unwrap()
    }

    #[test]
    fn finds_the_headers_counters() {
        assert_eq!(
            tracked_counters(HEADER),
            [
                "counter(heading)",
                "counter(math.equation)",
                "counter(\"thm\")"
            ]
        );
    }

    /// The chained end state of every block equals the counters at the same
    /// place in a compile of the whole document.
    #[test]
    fn chained_counters_match_a_full_compile() {
        let tracked = tracked_counters(HEADER);
        let mut world = BridgeWorld::new();

        let mut chained = vec![];
        let mut state = initial_state(&tracked);
        for snippet in BLOCKS {
            let (prefix, suffix) = chain_code(&tracked, &state);
            let around = Around {
                header: HEADER.into(),
                prefix,
                suffix,
            };
            let end = end_state(&compile_block(&mut world, &around, snippet)).unwrap();
            chained.push(end.clone());
            state = end;
        }

        // The whole document, with a probe after every block.
        let gets: Vec<String> = tracked.iter().map(|e| format!("{e}.get()")).collect();
        let mut src = format!("{HEADER}\n");
        for (i, snippet) in BLOCKS.iter().enumerate() {
            src.push_str(&format!(
                "{snippet}\n#context [#metadata(({},)) <probe-{i}>]\n\n",
                gets.join(", ")
            ));
        }
        let doc = compile(&mut world, &src).unwrap();
        for (i, expected) in chained.iter().enumerate() {
            let label = Label::new(PicoStr::intern(&format!("probe-{i}"))).unwrap();
            let probe = doc.introspector().query_label(label).unwrap();
            let value = &probe.to_packed::<MetadataElem>().unwrap().value;
            let full: CounterState = match value {
                Value::Array(cs) => cs
                    .iter()
                    .map(|c| match c {
                        Value::Array(ls) => ls.iter().map(|v| v.clone().cast().unwrap()).collect(),
                        _ => panic!("not an array"),
                    })
                    .collect(),
                _ => panic!("not an array"),
            };
            assert_eq!(&full, expected, "after block {i}: {}", BLOCKS[i]);
        }
        // Spot check: "1.2" after the second section's subsection.
        assert_eq!(chained[6][0], vec![2, 1]);
        assert_eq!(chained[8][1], vec![3]);
        assert_eq!(chained[8][2], vec![2]);
    }

    /// Restoring and reporting counters adds no visible content.
    #[test]
    fn chain_code_does_not_change_the_render() {
        let tracked = tracked_counters(HEADER);
        let mut world = BridgeWorld::new();
        let start = vec![vec![2, 3], vec![5], vec![1]];
        for snippet in ["= Intro", "== Sub", "$ a = b $", "Some *raw* text"] {
            let plain = Around {
                header: HEADER.into(),
                ..Around::default()
            };
            let (prefix, suffix) = chain_code(&tracked, &start);
            let chained = Around {
                prefix,
                suffix,
                ..plain.clone()
            };
            let a = compile_block(&mut world, &plain, snippet);
            let b = compile_block(&mut world, &chained, snippet);
            assert_eq!(a.pages().len(), 1, "{snippet}");
            assert_eq!(b.pages().len(), 1, "{snippet}");
            let (fa, fb) = (&a.pages()[0].frame, &b.pages()[0].frame);
            assert_eq!(fa.size(), fb.size(), "{snippet}");
            assert_eq!(fa.baseline(), fb.baseline(), "{snippet}");
        }
    }
}
