//! `.sem` files: a zip of the project. `document.json` holds a format
//! version, the Typst version the document was written with, the header
//! and the blocks (with their ids; other nodes' ids are regenerated on load);
//! `remarks.json` the remarks (if any, see [`remarks_to_json`]);
//! `main.typ` is the document as Typst, written on every save so the
//! unzipped folder compiles as is (not read back); every other entry is a
//! project file at its path (`images/chain.png`), except the version
//! history under `.git/` (see [`History`]).

use std::io::{Cursor, Read, Write};

use model::{BlockNode, Document};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::history::{GIT_DIR, History};

/// Version of `document.json` written by this build (restarted at 1 with
/// the zipped format, 2026-11-12).
pub const FORMAT_VERSION: u32 = 1;

/// The entries of a project zip that are not project files.
const DOCUMENT: &str = "document.json";
const MAIN: &str = "main.typ";
const REMARKS: &str = "remarks.json";

/// A remark in `remarks.json`: on block `block`, or on the range between
/// two caret positions in it (paths from the block; the inner nodes' ids
/// are not saved).
#[derive(Serialize, Deserialize)]
struct RemarkFile {
    id: u64,
    block: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<PointFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end: Option<PointFile>,
    text: String,
    author: String,
    date: String,
    #[serde(default)]
    resolved: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    replies: Vec<ReplyFile>,
}

#[derive(Serialize, Deserialize)]
struct ReplyFile {
    text: String,
    author: String,
    date: String,
}

#[derive(Serialize, Deserialize)]
struct PointFile {
    path: Vec<model::Step>,
    offset: usize,
}

/// `remarks.json` for a document, `None` without remarks.
pub fn remarks_to_json(doc: &Document) -> Option<String> {
    if doc.remarks.is_empty() {
        return None;
    }
    let point = |c: model::CursorPath| PointFile {
        path: c.path,
        offset: c.offset,
    };
    let remarks: Vec<RemarkFile> = doc
        .remarks
        .iter()
        .map(|r| {
            let range = doc.remark_range(r);
            let (start, end) = match range {
                Some((a, b)) => (Some(point(a)), Some(point(b))),
                None => (None, None),
            };
            RemarkFile {
                id: r.id,
                block: r.anchor.block().0,
                start,
                end,
                text: r.text.clone(),
                author: r.author.clone(),
                date: r.date.clone(),
                resolved: r.resolved,
                replies: r
                    .replies
                    .iter()
                    .map(|a| ReplyFile {
                        text: a.text.clone(),
                        author: a.author.clone(),
                        date: a.date.clone(),
                    })
                    .collect(),
            }
        })
        .collect();
    let mut out = serde_json::to_string_pretty(&remarks).expect("remarks always serialize");
    out.push('\n');
    Some(out)
}

/// Read `remarks.json` into a loaded document. A range that no longer
/// resolves is on its block.
pub fn remarks_from_json(doc: &mut Document, text: &str) -> Result<(), String> {
    let remarks: Vec<RemarkFile> =
        serde_json::from_str(text).map_err(|e| format!("not valid remarks: {e}"))?;
    for r in remarks {
        let block = model::NodeId(r.block);
        doc.reserve_id(block);
        let point = |p: &PointFile| {
            doc.point_at(&model::CursorPath {
                block,
                path: p.path.clone(),
                offset: p.offset,
            })
        };
        let range = match (&r.start, &r.end) {
            (Some(a), Some(b)) => point(a).zip(point(b)),
            _ => None,
        };
        let anchor = match range {
            Some((start, end)) => model::Anchor::Range { block, start, end },
            None => model::Anchor::Block(block),
        };
        doc.remarks.push(model::Remark {
            id: r.id,
            anchor,
            text: r.text,
            author: r.author,
            date: r.date,
            resolved: r.resolved,
            replies: r
                .replies
                .into_iter()
                .map(|a| model::Reply {
                    text: a.text,
                    author: a.author,
                    date: a.date,
                })
                .collect(),
        });
    }
    Ok(())
}

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

/// Parse `document.json`. Blocks keep their saved ids, the other nodes get
/// fresh ones; the structural invariants are restored; no files yet.
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
    doc.restore_ids();
    doc.check_invariants()?;
    Ok(doc)
}

/// The document as a Typst project: `main.typ`, the bibliography's source
/// as `refs.bib`, and the files `main.typ` names (a picture deleted since
/// stays out; the document keeps it, for undo), by path. What the zip holds
/// besides `document.json`, and what Export Typst writes.
pub fn typst_project(doc: &Document) -> Vec<(String, Vec<u8>)> {
    let main = typst_bridge::serialize_document(doc);
    let bib = typst_bridge::bib::BIB_FILE;
    let source = doc.blocks.iter().find_map(|b| match &b.kind {
        model::Block::Bibliography(source) => Some(format!("{source}\n")),
        _ => None,
    });
    let files = doc
        .files
        .iter()
        .filter(|(p, _)| main.contains(p) && safe_path(p) && *p != bib && !model::is_scratch(p))
        .map(|(p, b)| (p.to_string(), b.to_vec()));
    std::iter::once((MAIN.to_string(), main.clone().into_bytes()))
        .chain(source.map(|s| (bib.to_string(), s.into_bytes())))
        .chain(files)
        .collect()
}

/// The files `main.typ` reads (see [`typst_project`]), and the scratch
/// files it names, to compile it (a draft PDF shows draft figures).
pub fn project_files(doc: &Document) -> model::Files {
    let mut project = typst_project(doc).into_iter();
    let main = project.next().map(|(_, m)| m).unwrap_or_default();
    let main = String::from_utf8_lossy(&main);
    let mut files = model::Files::default();
    for (path, bytes) in project {
        files.insert(path, bytes);
    }
    for (path, bytes) in doc.files.iter() {
        if model::is_scratch(path) && main.contains(path) {
            files.insert(path, bytes.to_vec());
        }
    }
    files
}

/// Whether `path` stays inside the project: relative, no `..`.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| part != ".." && part != "." && !part.is_empty())
}

/// What a version of the document holds: `document.json`, its remarks (if
/// any) and its Typst project.
pub fn snapshot(doc: &Document) -> Vec<(String, Vec<u8>)> {
    std::iter::once((DOCUMENT.to_string(), to_json(doc).into_bytes()))
        .chain(remarks_to_json(doc).map(|r| (REMARKS.to_string(), r.into_bytes())))
        .chain(typst_project(doc))
        .collect()
}

/// The project zip of a document: its [`snapshot`] and its `history` (if
/// it has one).
pub fn to_zip(doc: &Document, history: Option<&History>) -> Result<Vec<u8>, String> {
    let git = history
        .map(History::entries)
        .transpose()?
        .unwrap_or_default();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    let mut add = |path: &str, bytes: &[u8]| -> zip::result::ZipResult<()> {
        zip.start_file(path, options)?;
        zip.write_all(bytes)?;
        Ok(())
    };
    let written = (|| {
        for (path, bytes) in snapshot(doc) {
            add(&path, &bytes)?;
        }
        // Scratch files: kept, not versioned.
        for (path, bytes) in doc.files.iter() {
            if model::is_scratch(path) && safe_path(path) {
                add(path, bytes)?;
            }
        }
        for (path, bytes) in &git {
            add(&format!("{GIT_DIR}{path}"), bytes)?;
        }
        Ok::<_, zip::result::ZipError>(())
    })();
    written.map_err(|e| e.to_string())?;
    let bytes = zip.finish().map_err(|e| e.to_string())?.into_inner();
    Ok(bytes)
}

/// The document of a version's files (a [`snapshot`]).
pub fn from_snapshot(entries: Vec<(String, Vec<u8>)>) -> Result<Document, String> {
    let (mut json, mut remarks) = (None, None);
    let mut files = model::Files::default();
    for (path, bytes) in entries {
        match path.as_str() {
            DOCUMENT => json = Some(bytes),
            REMARKS => remarks = Some(bytes),
            MAIN => {}
            _ if safe_path(&path) => files.insert(path, bytes),
            _ => {}
        }
    }
    let json = json.ok_or("the version has no document.json")?;
    let text = std::str::from_utf8(&json).map_err(|_| "document.json is not UTF-8")?;
    let mut doc = from_json(text)?;
    read_remarks(&mut doc, remarks)?;
    doc.files = files;
    Ok(doc)
}

/// The remarks of `remarks.json` (if there is one) into `doc`.
fn read_remarks(doc: &mut Document, bytes: Option<Vec<u8>>) -> Result<(), String> {
    let Some(bytes) = bytes else {
        return Ok(());
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| "remarks.json is not UTF-8")?;
    remarks_from_json(doc, text)
}

/// Read a project zip: its document with its files, and its history (an
/// empty one if it has none).
pub fn from_zip(bytes: &[u8]) -> Result<(Document, History), String> {
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
    let (mut json, mut remarks) = (None, None);
    let mut files = model::Files::default();
    let mut git = vec![];
    for i in 0..count {
        let (name, bytes) = read(i)?;
        match name.as_str() {
            DOCUMENT => json = Some(bytes),
            REMARKS => remarks = Some(bytes),
            MAIN => {}
            _ if name.ends_with('/') => {}
            _ if !safe_path(&name) => return Err(format!("not a valid .sem file: entry {name}")),
            _ => match name.strip_prefix(GIT_DIR) {
                Some(path) => git.push((path.to_string(), bytes)),
                None => files.insert(name, bytes),
            },
        }
    }
    let json = json.ok_or("not a valid .sem file: no document.json")?;
    let text = std::str::from_utf8(&json).map_err(|_| "document.json is not UTF-8")?;
    let mut doc = from_json(text)?;
    read_remarks(&mut doc, remarks)?;
    doc.files = files;
    let history = match git.is_empty() {
        true => History::new()?,
        false => History::from_entries(&git)?,
    };
    Ok((doc, history))
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
        let pdf = compile_pdf(&serialize_document(&doc), project_files(&doc)).unwrap();
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
            "#figure(image(\"images/correlations.png\", width: 40%), caption: [Correlations in a \
             matrix product state decay exponentially.]) <fig:correlations>\n\n$ |x|",
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
        let opened = typst_bridge::import_project(&typ, project_files(&doc));
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
        let files = project_files(&doc);
        doc.files.insert("images/unused.svg", b"<svg/>".to_vec());
        doc.files.insert("scratch/scan.png", vec![1, 2]);
        let history = History::new().unwrap();
        history.commit(&snapshot(&doc), "Ada", "Saved").unwrap();
        let zip = to_zip(&doc, Some(&history)).unwrap();
        let (back, back_history) = from_zip(&zip).unwrap();
        assert_eq!(
            back_history.versions().unwrap(),
            history.versions().unwrap()
        );
        let version = back_history.versions().unwrap()[0].id;
        let old = from_snapshot(back_history.files_at(version).unwrap()).unwrap();
        assert_eq!(to_json(&old), to_json(&doc));
        assert_eq!(old.files, files, "no scratch file in a version");
        // Scratch files are kept in the zip, not exported.
        assert_eq!(back.files.get("scratch/scan.png"), Some(&[1u8, 2][..]));
        assert!(
            typst_project(&back)
                .iter()
                .all(|(p, _)| !p.starts_with("scratch/"))
        );
        assert_eq!(to_json(&back), to_json(&doc));
        let paths: Vec<&str> = files.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, ["images/correlations.png", "refs.bib"]);
        let mut kept = files.clone();
        kept.insert("scratch/scan.png", vec![1, 2]);
        assert_eq!(
            back.files, kept,
            "the files the document reads, and scratch files"
        );
        let mut archive = zip::ZipArchive::new(Cursor::new(&zip[..])).unwrap();
        let mut main = String::new();
        archive
            .by_name(MAIN)
            .unwrap()
            .read_to_string(&mut main)
            .unwrap();
        assert_eq!(main, serialize_document(&doc));
        assert!(from_zip(b"not a zip").is_err());
        for (path, safe) in [
            ("images/a.png", true),
            ("refs.bib", true),
            ("../a.png", false),
            ("/etc/x", false),
            ("a/../../b", false),
            ("a//b", false),
        ] {
            assert_eq!(safe_path(path), safe, "{path}");
        }
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

    /// Remarks are saved beside the document: a range keeps its text, a
    /// remark on a deleted block stays detached and its id is not reused.
    #[test]
    fn remarks_roundtrip() {
        let mut doc = sample::document();
        assert!(!snapshot(&doc).iter().any(|(p, _)| p == REMARKS));
        let (para, text) = doc
            .leaves()
            .iter()
            .find_map(|b| match b.kind.inlines()?.first()?.kind {
                model::Inline::Text(ref t) if t.chars().count() > 5 => Some((b.id, t.clone())),
                _ => None,
            })
            .unwrap();
        let at = |offset| model::CursorPath {
            block: para,
            path: vec![model::Step::Index(0)],
            offset,
        };
        let (start, end) = (doc.point_at(&at(1)).unwrap(), doc.point_at(&at(4)).unwrap());
        let range = model::Anchor::Range {
            block: para,
            start,
            end,
        };
        doc.add_remark(range, "Range.", "A", "2026-11-13");
        let block = doc.blocks[3].id;
        doc.add_remark(model::Anchor::Block(block), "Block.", "A", "2026-11-13");
        let gone = model::NodeId(10_000);
        doc.add_remark(model::Anchor::Block(gone), "Detached.", "A", "2026-11-13");
        doc.remarks[1].resolved = true;
        doc.remarks[1].replies.push(model::Reply {
            text: "Done.".into(),
            author: "B".into(),
            date: "2026-11-14".into(),
        });
        let zip = to_zip(&doc, None).unwrap();
        let (mut back, _) = from_zip(&zip).unwrap();
        assert_eq!(back.remarks.len(), 3);
        let (a, b) = back.remark_range(&back.remarks[0]).unwrap();
        assert_eq!((a.block, a.offset, b.offset), (para, 1, 4));
        assert_eq!(&text[1..4], {
            let t = back.block(para).unwrap().kind.inlines().unwrap()[0].clone();
            match t.kind {
                model::Inline::Text(t) => t[1..4].to_string(),
                _ => unreachable!(),
            }
        });
        assert_eq!(back.remarks[1].anchor, model::Anchor::Block(block));
        assert_eq!(back.remarks[1].text, "Block.");
        // (The range's text node gets a new id on load: compare the others.)
        assert_eq!(back.remarks[1..], doc.remarks[1..], "replies and resolved kept");
        assert_eq!(back.remarks[2].anchor, model::Anchor::Block(gone));
        assert!(back.fresh_id() > gone);
        back.check_invariants().unwrap();
    }
}
