//! `.sem` files: JSON with a format version, the Typst version the document
//! was written with, the header and the blocks. Node ids are not stored;
//! they are regenerated on load.

use model::{BlockNode, Document};
use serde::{Deserialize, Serialize};

/// Version of the file format written by this build. 2: the header.
/// 3: environments (`Env` blocks), with an optional `title` (absent when
/// empty, so older version-3 files load unchanged). 4: list items (`Item`).
/// 5: block calls with source arguments (`args` on `Env`, absent when empty).
/// 6: code and diagrams in formulas (`MathKind::Code`, `MathKind::Diagram`).
/// 7: function calls in formulas (`MathKind::Call`). 8: primes, text,
/// alignment points and line breaks in formulas. 9: calls in text
/// (`Inline::Call`: bold, italic, `#name[…]`). 10: inline code
/// (`Inline::Raw`). 11: block labels (`label`, absent when empty).
/// 12: references (`Inline::Ref`). 13: figure captions (`Block::Caption`).
/// 14: the bibliography (`Block::Bibliography`). 15: the title block
/// (`Block::Title`).
pub const FORMAT_VERSION: u32 = 15;

#[derive(Serialize)]
struct SemFileOut<'a> {
    version: u32,
    typst: &'a str,
    header: &'a str,
    blocks: &'a [BlockNode],
}

#[derive(Deserialize)]
struct SemFileIn {
    version: u32,
    #[allow(dead_code)]
    typst: String,
    /// Version 1 has none: the editor's defaults, nothing numbered.
    #[serde(default)]
    header: String,
    blocks: Vec<BlockNode>,
}

/// The file contents for a document (pretty-printed, ends with a newline).
pub fn to_json(doc: &Document) -> String {
    let file = SemFileOut {
        version: FORMAT_VERSION,
        typst: typst_bridge::TYPST_VERSION,
        header: &doc.header,
        blocks: &doc.blocks,
    };
    let mut out = serde_json::to_string_pretty(&file).expect("documents always serialize");
    out.push('\n');
    out
}

/// Parse a `.sem` file. The document gets fresh node ids and its structural
/// invariants restored.
pub fn from_json(text: &str) -> Result<Document, String> {
    let file: SemFileIn =
        serde_json::from_str(text).map_err(|e| format!("not a valid .sem file: {e}"))?;
    if file.version > FORMAT_VERSION {
        return Err(format!(
            "file format version {} is newer than this app supports ({FORMAT_VERSION})",
            file.version
        ));
    }
    let mut doc = Document::empty();
    doc.header = file.header;
    doc.blocks = file.blocks;
    doc.regenerate_ids();
    doc.check_invariants()?;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;
    use typst_bridge::{compile_pdf, serialize_document};

    /// The milestone's acceptance check: save → load → export gives the
    /// same Typst, and saving again gives the same file.
    #[test]
    fn save_load_export_roundtrip() {
        let doc = sample::document();
        let saved = to_json(&doc);
        let loaded = from_json(&saved).unwrap();
        assert_eq!(serialize_document(&loaded), serialize_document(&doc));
        assert_eq!(to_json(&loaded), saved);
        assert!(saved.contains(&format!("\"version\": {FORMAT_VERSION}")));
        assert!(saved.contains("\"header\": \"#set heading"));
        assert!(saved.contains(&format!("\"typst\": \"{}\"", typst_bridge::TYPST_VERSION)));
    }

    #[test]
    fn exported_typst_compiles_to_pdf() {
        let pdf = compile_pdf(&serialize_document(&sample::document())).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
    }

    /// The sample shows every feature: its typed section came out as typed.
    #[test]
    fn sample_has_every_feature() {
        let typ = serialize_document(&sample::document());
        let tail = &typ[typ.find("= Notation").unwrap()..];
        for part in [
            "$cal(H) = overline(z) + hat(x)$",
            "root(3, x)",
            "$f' (x) + g''$",
            "$x \"if\" y$",
            "$ket(ψ)$",
            "$D = #box(baseline: 30%, cetz.canvas(",
            "cases(x \"if\" x ≥ 0, - x \"otherwise\") $ <eq:abs>",
            "(b + c)^2 & = (b + c) (b + c) \\ & = b^2 + 2 b c + c^2 $ <eq:square>",
            "/ Gauge: an invertible matrix on a virtual leg.\n\n#align(center)[",
            "= Notation and markup <sec:notation>",
            "*bold*, _italic_, #underline[underlined]",
            "#link(\"https://typst.app\")[link];; code is `cargo test`.",
            "pages 3–7",
            "pause — like",
            "Fig.~1 together.\\ A line break",
            "<eq:abs>",
            "<eq:square>",
            "/ Bond dimension: the size",
            "#align(center)[",
            "@sec:mps, @eq:mps, @eq:abs, @fig:chain and @thm:fundamental.",
            "Footnotes#footnote[Numbered through the document; in the editor under their \
             paragraph.] are calls too.",
        ] {
            assert!(tail.contains(part), "missing {part:?}");
        }
        // A bibliography at the end, cited in the last paragraph.
        let doc = sample::document();
        assert!(matches!(
            doc.blocks.last().unwrap().kind,
            model::Block::Bibliography(_)
        ));
        assert_eq!(doc.bib_keys(), ["fannes1992", "perezgarcia2007"]);
        assert!(tail.contains("@fannes1992 and are reviewed in @perezgarcia2007."));
        // The references point to labels of the document.
        let last = doc.blocks[doc.blocks.len() - 2].kind.inlines().unwrap();
        let labels: Vec<&str> = doc.labels().iter().map(|(l, _)| *l).collect();
        let refs: Vec<&str> = last
            .iter()
            .filter_map(|i| match &i.kind {
                model::Inline::Ref(label) => Some(label.as_str()),
                _ => None,
            })
            .filter(|r| !doc.bib_keys().iter().any(|k| k == r))
            .collect();
        assert_eq!(
            refs,
            [
                "sec:mps",
                "eq:mps",
                "eq:abs",
                "fig:chain",
                "thm:fundamental"
            ]
        );
        assert!(typ.contains("caption: [A matrix product state on four sites.])["));
        assert!(refs.iter().all(|r| labels.contains(r)), "{labels:?}");
    }

    /// Opening an exported `.typ` gives the document back: it exports to
    /// the same Typst, with no raw blocks the sample did not have.
    #[test]
    fn exported_typst_opens_as_the_same_document() {
        let doc = sample::document();
        let typ = serialize_document(&doc);
        let opened = typst_bridge::import_typ(&typ);
        opened.check_invariants().unwrap();
        assert_eq!(serialize_document(&opened), typ);
        let raw = |d: &model::Document| {
            d.leaves()
                .iter()
                .filter(|b| matches!(b.kind, model::Block::RawTypst(_)))
                .count()
        };
        let raws: Vec<_> = opened
            .leaves()
            .into_iter()
            .filter(|b| matches!(b.kind, model::Block::RawTypst(_)))
            .collect();
        assert_eq!(raw(&opened), raw(&doc), "{raws:#?}");
    }

    #[test]
    fn bad_files_are_rejected() {
        assert!(from_json("{").is_err());
        assert!(from_json(r#"{"version": 99, "typst": "x", "blocks": []}"#).is_err());
        // An empty document gets its one paragraph back; version 1 has no
        // header.
        let doc = from_json(r#"{"version": 1, "typst": "0.15.1", "blocks": []}"#).unwrap();
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.header, "");
    }
}
