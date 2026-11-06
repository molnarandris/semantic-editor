//! Builds a small document, prints its `.sem` JSON, the derived Typst source
//! and the span map of the display formula, and checks that it compiles.

use model::Document;
use typst_bridge::{BridgeWorld, compile, serialize_block, serialize_document};

fn main() {
    let mut d = Document::empty();
    let t1 = d.text("The bound ");
    let (s, le, log, chi, two, plus, c) = (
        d.ident("S"),
        d.sym("≤"),
        d.ident("log"),
        d.sym("χ"),
        d.num("2"),
        d.sym("+"),
        d.ident("c"),
    );
    let f = d.frac(vec![log, chi], vec![two]);
    let m = d.math(vec![s, le, f, plus, c]);
    let t2 = d.text(" holds for *all* states.");
    let p = d.paragraph(vec![t1, m, t2]);
    let (x, i, n) = (d.ident("x"), d.ident("i"), d.ident("n"));
    let xi = d.scripts(vec![x], Some(vec![n]), Some(vec![i]));
    let slot_frac = d.frac(vec![xi], vec![]);
    let disp = d.display(vec![slot_frac]);
    d.blocks = vec![p, disp];

    println!(
        "--- .sem JSON ---\n{}\n",
        serde_json::to_string_pretty(&d).unwrap()
    );
    let typ = serialize_document(&d);
    println!("--- Typst ---\n{typ}");
    let (src, map) = serialize_block(&d.blocks[1]);
    println!("--- span map of `{src}` ---");
    for (id, range) in map.entries() {
        println!("{:>4} {:>8?}  {}", id.0, range, &src[range.clone()]);
    }
    match compile(&mut BridgeWorld::new(), &typ) {
        Ok(doc) => println!("\ncompiles: {} page(s)", doc.pages().len()),
        Err(e) => println!("\nERROR: {e:?}"),
    }
}
