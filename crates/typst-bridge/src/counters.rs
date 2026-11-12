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
/// equations, footnotes, and every `counter("…")` that `code` (header and
/// code blocks) names.
pub fn tracked_counters(code: &str) -> Vec<String> {
    let mut tracked = vec![
        "counter(heading)".to_string(),
        "counter(math.equation)".to_string(),
        "counter(footnote)".to_string(),
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

/// What a label points to, as a block that defines it reports: the counter
/// a reference to it shows (a Typst expression, as in [`CounterState`]) and
/// its value there.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LabelTarget {
    pub label: String,
    pub counter: String,
    pub value: Vec<i64>,
}

/// Label of the metadata that reports a block's labels.
const LABELS_LABEL: &str = "sem-labels";

/// Typst code reporting what `labels` (defined in the block) point to: for
/// each, the counter a reference shows and its value, or `none` if it
/// cannot be referenced (no numbering, defined twice, or not a heading,
/// equation or figure).
pub fn label_report(labels: &[String]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let items: Vec<String> = labels
        .iter()
        .map(|l| format!("label({})", quoted(l)))
        .collect();
    format!(
        "\n#context [#metadata(({},).map(l => {{
  let found = query(l)
  if found.len() != 1 {{ return none }}
  let el = found.first()
  let f = el.func()
  let expr = if f == heading {{ \"counter(heading)\" }} else if f == math.equation {{
    \"counter(math.equation)\"
  }} else if f == figure {{
    \"counter(figure.where(kind: \" + repr(el.kind) + \"))\"
  }} else {{ return none }}
  if el.numbering == none {{ return none }}
  (str(l), expr, eval(expr).at(el.location()))
}})) <{LABELS_LABEL}>]",
        items.join(", ")
    )
}

/// The labels a compiled block reported (see [`label_report`]); those that
/// cannot be referenced are left out.
pub fn label_targets(doc: &PagedDocument) -> Vec<LabelTarget> {
    let Some(label) = Label::new(PicoStr::intern(LABELS_LABEL)) else {
        return vec![];
    };
    let Ok(content) = doc.introspector().query_label(label) else {
        return vec![];
    };
    let Some(Value::Array(items)) = content.to_packed::<MetadataElem>().map(|m| &m.value) else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| {
            let Value::Array(item) = item else {
                return None;
            };
            let [Value::Str(label), Value::Str(counter), Value::Array(value)] = item.as_slice()
            else {
                return None;
            };
            let value = value
                .iter()
                .map(|v| match v {
                    Value::Int(n) => Some(*n),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(LabelTarget {
                label: label.to_string(),
                counter: counter.to_string(),
                value,
            })
        })
        .collect()
}

/// Typst code, after a block, making `target` resolvable there: an
/// invisible `element` (a heading, equation or environment call like the
/// one labelled) with the label, its counter set so the element gets the
/// target's number. References in the block then render as in the full
/// document, with the header's supplements and numbering.
pub fn ref_stub(target: &LabelTarget, element: &str) -> String {
    let mut value = target.value.clone();
    if let Some(last) = value.last_mut() {
        *last -= 1; // the element steps the counter back up
    }
    let items: Vec<String> = value.iter().map(i64::to_string).collect();
    let comma = if items.len() == 1 { "," } else { "" };
    format!(
        "\n#{}.update(({}{comma}))\n#place(hide[{element} <{}>])",
        target.counter,
        items.join(", "),
        target.label
    )
}

/// An invisible element like the one `target` labels, known only from its
/// counter (a label in raw Typst): a heading, an equation, or a figure of a
/// built-in kind. `None` for other figures (their supplement is unknown).
pub fn stand_in(target: &LabelTarget) -> Option<String> {
    match target.counter.as_str() {
        "counter(heading)" => Some(format!("#heading(level: {})[]", target.value.len())),
        "counter(math.equation)" => Some("#math.equation(block: true)[]".to_string()),
        c => {
            let kind = c
                .strip_prefix("counter(figure.where(kind: ")?
                .strip_suffix("))")?;
            ["image", "table", "raw"]
                .contains(&kind)
                .then(|| format!("#figure(kind: {kind})[]"))
        }
    }
}

/// Typst code, before a block, showing references to `label` as a red
/// `@label` (a missing, unnumbered or duplicate target) instead of failing.
pub fn missing_ref(label: &str) -> String {
    format!(
        "#show ref.where(target: label({})): _ => text(fill: rgb(\"#c01c28\"), {})\n",
        quoted(label),
        quoted(&format!("@{label}"))
    )
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
                "counter(footnote)",
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
                ..Around::default()
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

    /// The text of a page, hidden content left out.
    fn page_text(frame: &typst::layout::Frame, out: &mut String) {
        for (_, item) in frame.items() {
            match item {
                typst::layout::FrameItem::Group(g) => page_text(&g.frame, out),
                typst::layout::FrameItem::Text(t) => out.push_str(&t.text),
                _ => {}
            }
        }
    }

    /// Labelled blocks report their targets; a block referencing them with
    /// stand-in targets shows the references as in the full document, and
    /// the stand-ins take no space; a missing label shows in red.
    #[test]
    fn references_through_stand_ins() {
        let mut world = BridgeWorld::new();
        let report = |labels: &[&str]| {
            label_report(&labels.iter().map(|l| l.to_string()).collect::<Vec<_>>())
        };
        let mut targets = vec![];
        for (prefix, snippet, label) in [
            ("", "= One\n\n== Two <sec:two>", "sec:two"),
            ("#counter(math.equation).update((2,))\n", "$ x $ <eq>", "eq"),
            ("", "#lemma[Claim.] <lem>", "lem"),
        ] {
            let around = Around {
                header: HEADER.into(),
                prefix: prefix.into(),
                suffix: report(&[label]),
                ..Around::default()
            };
            targets.extend(label_targets(&compile_block(&mut world, &around, snippet)));
        }
        let want = |label: &str, counter: &str, value: &[i64]| LabelTarget {
            label: label.into(),
            counter: counter.into(),
            value: value.to_vec(),
        };
        assert_eq!(
            targets,
            [
                want("sec:two", "counter(heading)", &[1, 1]),
                want("eq", "counter(math.equation)", &[3]),
                want("lem", "counter(figure.where(kind: \"lemma\"))", &[1]),
            ]
        );
        let elements = [
            "#heading(level: 2)[]",
            "#math.equation(block: true)[]",
            "#lemma[]",
        ];
        let around = Around {
            header: HEADER.into(),
            prefix: missing_ref("gone"),
            suffix: targets
                .iter()
                .zip(elements)
                .map(|(t, e)| ref_stub(t, e))
                .collect(),
            ..Around::default()
        };
        let doc = compile_block(&mut world, &around, "See @sec:two, @eq, @lem and @gone.");
        let mut text = String::new();
        page_text(&doc.pages()[0].frame, &mut text);
        // The same references in one whole document.
        let whole = compile_block(
            &mut world,
            &Around {
                header: HEADER.into(),
                ..Around::default()
            },
            "= One\n\n== Two <sec:two>\n\n#counter(math.equation).update((2,))\n$ x $ <eq>\n\n\
             #lemma[Claim.] <lem>\n\nSee @sec:two, @eq, @lem and",
        );
        let mut full = String::new();
        page_text(&whole.pages()[0].frame, &mut full);
        let refs = &full[full.rfind("See").unwrap()..];
        assert_eq!(format!("{refs} @gone."), text);
        let plain = compile_block(
            &mut world,
            &Around {
                header: HEADER.into(),
                ..Around::default()
            },
            &format!("{} \\@gone.", refs.replace('@', "\\@")),
        );
        assert_eq!(doc.pages()[0].frame.size(), plain.pages()[0].frame.size());
    }
}
