//! Print the syntax tree of the Typst source given as the argument.
fn main() {
    let src = std::env::args().nth(1).unwrap();
    println!("{:#?}", typst::syntax::parse(&src));
}
