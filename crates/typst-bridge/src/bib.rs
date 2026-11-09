//! The bibliography: Typst code for an embedded source, and the hidden
//! citations and bibliography that let a block render its citations (or the
//! bibliography render its list) as in the whole document.

use crate::serialize::quoted;

/// The Typst expression for an embedded bibliography source (BibTeX or
/// Hayagriva YAML): its text in a raw block, so it stays verbatim. The fence
/// is longer than any run of backticks in the source.
pub fn bib_bytes(source: &str) -> String {
    let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("bytes({fence}bib\n{source}\n{fence}.text)")
}

/// The bibliography block, `#bibliography(bytes(```bib …```.text))`.
pub fn bibliography_code(source: &str) -> String {
    format!("#bibliography({})", bib_bytes(source))
}

/// Typst code before a block: citations of `keys`, hidden and taking no
/// space, so the block's own citations are numbered after them (as after the
/// document's earlier citations), or the bibliography lists them.
pub fn hidden_cites(keys: &[String]) -> String {
    if keys.is_empty() {
        return String::new();
    }
    let cites: String = keys
        .iter()
        .map(|k| format!("#cite(label({}))", quoted(k)))
        .collect();
    format!("#place(hide[{cites}])\n")
}

/// Typst code after a citing block: the bibliography, hidden, so its
/// citations resolve.
pub fn hidden_bibliography(source: &str) -> String {
    format!("\n#place(hide(bibliography({})))", bib_bytes(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Around, BridgeWorld, block_document, compile};

    const BIB: &str = "@article{alpha,
  title = {First Paper},
  author = {Alpha, Ann},
  journal = {J. Tests},
  year = {2020},
}
@book{beta,
  title = {Second Book},
  author = {Beta, Bob},
  publisher = {Press},
  year = {2021},
}";

    fn text_of(world: &mut BridgeWorld, around: &Around, snippet: &str) -> (String, f64) {
        fn walk(frame: &typst::layout::Frame, out: &mut String) {
            for (_, item) in frame.items() {
                match item {
                    typst::layout::FrameItem::Group(g) => walk(&g.frame, out),
                    typst::layout::FrameItem::Text(t) => out.push_str(&t.text),
                    _ => {}
                }
            }
        }
        let (src, _) = block_document(around, snippet, 300.0);
        let doc = compile(world, &src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
        let mut out = String::new();
        walk(&doc.pages()[0].frame, &mut out);
        (out, doc.pages()[0].frame.height().to_pt())
    }

    /// A citing block with the earlier citations hidden before it and the
    /// bibliography hidden after it renders its citations as in the whole
    /// document, taking no more space; the bibliography with all citations
    /// hidden before it lists what the document cites.
    #[test]
    fn citations_through_hidden_ones() {
        let mut world = BridgeWorld::new();
        let bib = bibliography_code(BIB);
        let whole = format!("First @beta.\n\nThen @alpha and @beta.\n\n{bib}");
        let (full, _) = text_of(&mut world, &Around::default(), &whole);
        // The second paragraph alone, after a citation of beta.
        let around = Around {
            prefix: hidden_cites(&["beta".into()]),
            suffix: hidden_bibliography(BIB),
            ..Around::default()
        };
        let (alone, height) = text_of(&mut world, &around, "Then @alpha and @beta.");
        assert!(full.contains(&alone), "{alone:?} not in {full:?}");
        let (_, plain) = text_of(&mut world, &Around::default(), "Then [1] and [2].");
        assert!((height - plain).abs() < 0.01, "{height} vs {plain}");
        // The bibliography alone, after all the document's citations.
        let around = Around {
            prefix: hidden_cites(&["beta".into(), "alpha".into()]),
            ..Around::default()
        };
        let (list, _) = text_of(&mut world, &around, &bib);
        assert!(full.ends_with(&list), "{list:?} not at the end of {full:?}");
    }

    #[test]
    fn fences_outgrow_backticks() {
        assert!(bib_bytes("a ```` b").contains("`````bib"));
        assert!(bib_bytes("plain").starts_with("bytes(```bib\n"));
    }
}
