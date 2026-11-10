//! Import a `.typ` file and list the blocks that came out as raw Typst
//! (their first line), to see what the importer does not handle yet.
//!
//!   build-aux/sdk cargo run -p typst-bridge --example import_report -- FILE.typ

fn main() {
    let path = std::env::args().nth(1).expect("a .typ file");
    let source = std::fs::read_to_string(path).expect("readable");
    let doc = typst_bridge::import_typ(&source);
    std::fs::write("target/import-check/header.typ", &doc.header).unwrap();
    std::fs::write(
        "target/import-check/export.typ",
        typst_bridge::serialize_document(&doc),
    )
    .unwrap();
    let mut raw = 0;
    let mut total = 0;
    for b in doc.leaves() {
        total += 1;
        if let model::Block::RawTypst(s) = &b.kind {
            raw += 1;
            let first: String = s.lines().next().unwrap_or("").chars().take(90).collect();
            let code = if typst_bridge::is_code(s) {
                "code"
            } else {
                "raw "
            };
            println!("{code} | {first}");
            for name in [
                "#theorem",
                "#lemma",
                "#proposition",
                "#definition",
                "#proof",
                "#remark",
                "#corollary",
            ] {
                if let Some(at) = s.find(name) {
                    let line = s[..at].matches('\n').count();
                    println!(
                        "       contains {name} at line {line} of {}",
                        s.lines().count()
                    );
                }
            }
        }
    }
    let mut envs = std::collections::BTreeMap::new();
    fn walk(b: &model::BlockNode, envs: &mut std::collections::BTreeMap<String, usize>) {
        if let model::Block::Env { name, .. } = &b.kind {
            *envs.entry(name.clone()).or_insert(0) += 1;
        }
        for c in b.kind.body().into_iter().flatten() {
            walk(c, envs);
        }
    }
    doc.blocks.iter().for_each(|b| walk(b, &mut envs));
    println!("environments: {envs:?}");
    println!("{raw} raw of {total} blocks");
    let args: Vec<String> = std::env::args().collect();
    if let Some(k) = args.iter().position(|a| a == "--against") {
        let other = std::fs::read_to_string(&args[k + 1]).expect("readable");
        let mut other_doc = typst_bridge::import_typ(&other);
        other_doc.header = String::new();
        other_doc.blocks = vec![other_doc.raw(&other)];
        compare(&source, &other_doc);
        return;
    }
    if std::env::args().any(|a| a == "--compare") {
        compare(&source, &doc);
    }
}

/// With `--compare`: whether importing then exporting renders the same pages.
fn compare(source: &str, doc: &model::Document) {
    let pages = |src: &str| -> Vec<typst_bridge::Raster> {
        let mut world = typst_bridge::BridgeWorld::new();
        match typst_bridge::compile(&mut world, src) {
            Ok(d) => d
                .pages()
                .iter()
                .map(|p| typst_bridge::render_page(p, 1.5))
                .collect(),
            Err(e) => {
                std::fs::write("target/import-check/failed.typ", src).unwrap();
                world.set_main_text(src);
                let result = typst::compile::<typst_layout::PagedDocument>(&world);
                if let Err(errors) = result.output {
                    for err in errors.iter().take(3) {
                        let line = typst::WorldExt::range(&world, err.span)
                            .map(|r| src[..r.start].matches('\n').count() + 1);
                        let trace: Vec<_> = err
                            .trace
                            .iter()
                            .filter_map(|t| typst::WorldExt::range(&world, t.span))
                            .map(|r| src[..r.start].matches('\n').count() + 1)
                            .collect();
                        println!(
                            "error at line {line:?} (trace lines {trace:?}): {}",
                            err.message
                        );
                    }
                }
                panic!("does not compile: {:?}", &e[..e.len().min(3)])
            }
        }
    };
    let before = pages(&format!("{}{source}", typst_bridge::PREAMBLE));
    let after = pages(&typst_bridge::serialize_document(doc));
    let differ: Vec<usize> = (0..before.len().max(after.len()))
        .filter(|&i| before.get(i).map(|r| &r.data) != after.get(i).map(|r| &r.data))
        .map(|i| i + 1)
        .collect();
    println!(
        "pages: {} before, {} after; differing: {differ:?}",
        before.len(),
        after.len()
    );
    // The first differing page, before and after, as PNGs.
    for &page in differ.iter().take(4) {
        for (name, r) in [("before", &before[page - 1]), ("after", &after[page - 1])] {
            let size = tiny_skia::IntSize::from_wh(r.width, r.height).unwrap();
            let pixmap = tiny_skia::Pixmap::from_vec(r.data.clone(), size).unwrap();
            pixmap
                .save_png(format!("target/import-check/{name}-{page}.png"))
                .unwrap();
        }
    }
}
