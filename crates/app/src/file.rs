//! `.sem` files: a zip of the project. `document.json` holds a format
//! version, the Typst version the document was written with, the header
//! and the blocks (node ids are not stored; they are regenerated on load);
//! `main.typ` is the document as Typst, written on every save so the
//! unzipped folder compiles as is (not read back); every other entry is a
//! project file at its path (`images/chain.png`).

use std::io::{Cursor, Read, Write};

use model::{BlockNode, Document};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

/// Version of `document.json` written by this build (restarted at 1 with
/// the zipped format, 2026-11-12).
pub const FORMAT_VERSION: u32 = 1;

/// The entries of a project zip that are not project files.
const DOCUMENT: &str = "document.json";
const MAIN: &str = "main.typ";

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
    #[serde(default)]
    header: String,
    blocks: Vec<BlockNode>,
}

/// `document.json` for a document (pretty-printed, ends with a newline).
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

/// Parse `document.json`. The document gets fresh node ids and its
/// structural invariants restored; it has no files yet.
pub fn from_json(text: &str) -> Result<Document, String> {
    let file: SemFileIn =
        serde_json::from_str(text).map_err(|e| format!("not a valid document: {e}"))?;
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

/// The project zip of a document: `document.json`, `main.typ` and its
/// files.
pub fn to_zip(doc: &Document) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    let mut add = |path: &str, bytes: &[u8]| -> zip::result::ZipResult<()> {
        zip.start_file(path, options)?;
        zip.write_all(bytes)?;
        Ok(())
    };
    let written = (|| {
        add(DOCUMENT, to_json(doc).as_bytes())?;
        add(MAIN, typst_bridge::serialize_document(doc).as_bytes())?;
        for (path, bytes) in doc.files.iter() {
            add(path, bytes)?;
        }
        Ok::<_, zip::result::ZipError>(())
    })();
    written.map_err(|e| e.to_string())?;
    let bytes = zip.finish().map_err(|e| e.to_string())?.into_inner();
    Ok(bytes)
}

/// Read a project zip: its document with its files.
pub fn from_zip(bytes: &[u8]) -> Result<Document, String> {
    let not_sem = |e: zip::result::ZipError| format!("not a valid .sem file: {e}");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(not_sem)?;
    let count = zip.len();
    let mut read = |i: usize| -> Result<(String, Vec<u8>), String> {
        let mut entry = zip.by_index(i).map_err(not_sem)?;
        let name = entry.name().map_err(not_sem)?.into_owned();
        let mut bytes = vec![];
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        Ok((name, bytes))
    };
    let mut json = None;
    let mut files = model::Files::default();
    for i in 0..count {
        let (name, bytes) = read(i)?;
        match name.as_str() {
            DOCUMENT => json = Some(bytes),
            MAIN => {}
            _ if name.ends_with('/') => {}
            _ => files.insert(name, bytes),
        }
    }
    let json = json.ok_or("not a valid .sem file: no document.json")?;
    let text = std::str::from_utf8(&json).map_err(|_| "document.json is not UTF-8")?;
    let mut doc = from_json(text)?;
    doc.files = files;
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
        let doc = sample::document();
        let pdf = compile_pdf(&serialize_document(&doc), doc.files.clone()).unwrap();
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

    /// A project zip holds the document, its Typst and its files, and
    /// reads back as the same document with the same files.
    #[test]
    fn projects_are_zips() {
        let mut doc = sample::document();
        doc.files.insert("images/x.svg", b"<svg/>".to_vec());
        let zip = to_zip(&doc).unwrap();
        let back = from_zip(&zip).unwrap();
        assert_eq!(to_json(&back), to_json(&doc));
        assert_eq!(back.files, doc.files);
        let mut archive = zip::ZipArchive::new(Cursor::new(&zip[..])).unwrap();
        let mut main = String::new();
        archive
            .by_name(MAIN)
            .unwrap()
            .read_to_string(&mut main)
            .unwrap();
        assert_eq!(main, serialize_document(&doc));
        assert!(from_zip(b"not a zip").is_err());
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
