//! Counter state chained through blocks.
//!
//! Every block compiles as its own document, so Typst counters (heading
//! numbers, equation numbers, theorem numbers, the header's own counters)
//! would restart in each one. A counter-sensitive block therefore gets a
//! prefix that sets the counters to their values at its start, and a suffix
//! that reports their values at its end as labelled metadata, which the next
//! sensitive block starts from.
//!
//! The state is a list of counters as Typst expressions with their values.
//! It starts with the counters named in the document's code; the suffix adds
//! the counter of every figure kind the block contains
//! (`counter(figure.where(kind: "lemma"))`), so theorems built as figures are
//! numbered on without the editor knowing their kinds.

use typst::foundations::{Label, Value};
use typst::introspection::{Introspector, MetadataElem};
use typst::utils::PicoStr;
use typst_layout::PagedDocument;

use crate::serialize::quoted;

/// Counters (Typst expressions) and their values.
pub type CounterState = Vec<(String, Vec<i64>)>;

/// Label of the metadata that reports a block's end state.
const END_LABEL: &str = "sem-end";

/// The counters to carry from block to block from the start: headings,
/// equations, and every `counter("…")` that `code` (header and code blocks)
/// names.
pub fn tracked_counters(code: &str) -> Vec<String> {
    let mut tracked = vec![
        "counter(heading)".to_string(),
        "counter(math.equation)".to_string(),
    ];
    let mut rest = code;
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
    tracked.iter().map(|e| (e.clone(), vec![0])).collect()
}

/// Typst code around a sensitive block's snippet: the prefix sets the
/// counters to `start`, the suffix reports them (and the counters of the
/// block's figure kinds) at the end.
pub fn chain_code(start: &CounterState) -> (String, String) {
    let mut prefix = String::new();
    for (expr, value) in start {
        let items: Vec<String> = value.iter().map(i64::to_string).collect();
        // `(3,)` is a one-element array, `(3)` just a number.
        let comma = if items.len() == 1 { "," } else { "" };
        prefix.push_str(&format!("#{expr}.update(({}{comma}))\n", items.join(", ")));
    }
    let known: Vec<String> = start.iter().map(|(e, _)| quoted(e)).collect();
    let suffix = format!(
        "\n#context [#metadata({{
  let known = ({},)
  for f in query(figure) {{
    let e = \"counter(figure.where(kind: \" + repr(f.kind) + \"))\"
    if e not in known {{ known.push(e) }}
  }}
  known.map(e => (e, eval(e).get()))
}}) <{END_LABEL}>]",
        known.join(", ")
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
        .map(|pair| {
            let Value::Array(pair) = pair else {
                return None;
            };
            let [Value::Str(expr), Value::Array(levels)] = pair.as_slice() else {
                return None;
            };
            let levels = levels
                .iter()
                .map(|v| match v {
                    Value::Int(n) => Some(*n),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some((expr.to_string(), levels))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Around, BridgeWorld, block_document, compile};

    /// Numbered headings and equations, a custom counter, and theorem
    /// environments built as figures of their own kinds (as real papers do).
    const HEADER: &str = "\
#set heading(numbering: \"1.1\")
#set math.equation(numbering: \"(1)\")
#let thm-count = counter(\"thm\")
#let env(kind, name) = body => figure(kind: kind, supplement: name, numbering: \"1\", body)
#let lemma = env(\"lemma\", \"Lemma\")
#let theorem = env(\"theorem\", \"Theorem\")
";

    /// Counter-sensitive blocks of a small document.
    const BLOCKS: &[&str] = &[
        "= Intro",
        "$ a = b $",
        "#lemma[First lemma.]",
        "== Details",
        "#counter(\"thm\").step()\n*Claim #context counter(\"thm\").display().*",
        "#lemma[Second lemma.]\n#theorem[A theorem.]",
        "$ c = d $",
        "= Next",
        "== More",
        "#theorem[Another theorem.]",
        "#counter(\"thm\").step()",
        "$ e = f $",
    ];

    fn compile_block(world: &mut BridgeWorld, around: &Around, snippet: &str) -> PagedDocument {
        let (src, _) = block_document(around, snippet, 300.0);
        compile(world, &src).unwrap()
    }

    #[test]
    fn finds_the_named_counters() {
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
    /// place in a compile of the whole document, theorem kinds included.
    #[test]
    fn chained_counters_match_a_full_compile() {
        let mut world = BridgeWorld::new();
        let mut chained = vec![];
        let mut state = initial_state(&tracked_counters(HEADER));
        for snippet in BLOCKS {
            let (prefix, suffix) = chain_code(&state);
            let around = Around {
                header: HEADER.into(),
                prefix,
                suffix,
            };
            state = end_state(&compile_block(&mut world, &around, snippet)).unwrap();
            chained.push(state.clone());
        }

        // The whole document, with a probe of the same counters after every
        // block.
        let mut src = format!("{HEADER}\n");
        for (i, (snippet, state)) in BLOCKS.iter().zip(&chained).enumerate() {
            let exprs: Vec<String> = state.iter().map(|(e, _)| quoted(e)).collect();
            src.push_str(&format!(
                "{snippet}\n#context [#metadata(({},).map(e => eval(e).get())) <probe-{i}>]\n\n",
                exprs.join(", ")
            ));
        }
        let doc = compile(&mut world, &src).unwrap();
        for (i, state) in chained.iter().enumerate() {
            let label = Label::new(PicoStr::intern(&format!("probe-{i}"))).unwrap();
            let probe = doc.introspector().query_label(label).unwrap();
            let Value::Array(values) = &probe.to_packed::<MetadataElem>().unwrap().value else {
                panic!("not an array");
            };
            let full: Vec<Vec<i64>> = values
                .iter()
                .map(|v| v.clone().cast::<Vec<i64>>().unwrap())
                .collect();
            let chain: Vec<Vec<i64>> = state.iter().map(|(_, v)| v.clone()).collect();
            assert_eq!(full, chain, "after block {i}: {}", BLOCKS[i]);
        }

        let value = |state: &CounterState, expr: &str| {
            state
                .iter()
                .find(|(e, _)| e == expr)
                .map(|(_, v)| v.clone())
        };
        let last = chained.last().unwrap();
        assert_eq!(value(last, "counter(heading)"), Some(vec![2, 1]));
        assert_eq!(value(last, "counter(math.equation)"), Some(vec![3]));
        assert_eq!(value(last, "counter(\"thm\")"), Some(vec![2]));
        let lemma = "counter(figure.where(kind: \"lemma\"))";
        let theorem = "counter(figure.where(kind: \"theorem\"))";
        assert_eq!(value(last, lemma), Some(vec![2]));
        assert_eq!(value(last, theorem), Some(vec![2]));
        // A kind is tracked from the block that first has one.
        assert_eq!(value(&chained[1], lemma), None);
        assert_eq!(value(&chained[2], lemma), Some(vec![1]));
    }

    /// Restoring and reporting counters adds no visible content.
    #[test]
    fn chain_code_does_not_change_the_render() {
        let mut world = BridgeWorld::new();
        let mut start = initial_state(&tracked_counters(HEADER));
        start[0].1 = vec![2, 3];
        start.push(("counter(figure.where(kind: \"lemma\"))".into(), vec![4]));
        for snippet in [
            "= Intro",
            "== Sub",
            "$ a = b $",
            "Some *raw* text",
            "#lemma[L.]",
        ] {
            let plain = Around {
                header: HEADER.into(),
                ..Around::default()
            };
            let (prefix, suffix) = chain_code(&start);
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
