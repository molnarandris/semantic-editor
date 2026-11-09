//! What a function called as a block (`#lemma[…]`, `#align(center)[…]`)
//! takes: read from the `#let`s of the header and the code blocks, else from
//! Typst's standard library. Parsing only, no compile, so it is cheap enough
//! to run on a keystroke.

use std::sync::LazyLock;

use model::CallShape;
use typst::foundations::Value;
use typst::syntax::ast::{self, Expr, LetBindingKind, Param};
use typst::syntax::parse;
use typst::{Library, LibraryExt};

static LIBRARY: LazyLock<Library> = LazyLock::new(Library::default);

/// The shape of function `name` as defined by `sources` (the header, then
/// the code blocks, in document order; a later definition wins), else by
/// the standard library. `None` if unknown: bound to something that is not
/// written as a function here (`#let theorem = thmbox(…)`), imported from a
/// package, or not defined at all.
pub fn call_shape(sources: &[&str], name: &str) -> Option<CallShape> {
    for source in sources.iter().rev() {
        if let Some(found) = defined_in(source, name) {
            return found;
        }
    }
    match LIBRARY.global.scope().get(name)?.read() {
        // Built-in functions take no title; their content is positional.
        Value::Func(func) => Some(CallShape {
            body: func.params().any(|p| p.positional()),
            title: false,
        }),
        _ => Some(CallShape {
            body: false,
            title: false,
        }),
    }
}

/// `Some(shape)` if a top-level `#let` in `source` binds `name` (the last
/// one wins); the shape is `None` unless it is written as a function.
fn defined_in(source: &str, name: &str) -> Option<Option<CallShape>> {
    let root = parse(source);
    let mut found = None;
    for node in root.children() {
        let Some(binding) = node.cast::<ast::LetBinding>() else {
            continue;
        };
        match binding.kind() {
            LetBindingKind::Closure(ident) if ident.as_str() == name => {
                let Some(Expr::Closure(closure)) = binding.init() else {
                    continue;
                };
                let mut positional = 0;
                let mut sink = false;
                for param in closure.params().children() {
                    match param {
                        Param::Pos(_) => positional += 1,
                        Param::Spread(_) => sink = true,
                        Param::Named(_) => {}
                    }
                }
                found = Some(Some(CallShape {
                    body: positional >= 1 || sink,
                    title: positional >= 2 || sink,
                }));
            }
            kind => {
                if kind.bindings().iter().any(|i| i.as_str() == name) {
                    found = Some(None);
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(body: bool, title: bool) -> Option<CallShape> {
        Some(CallShape { body, title })
    }

    #[test]
    fn shapes_from_lets_and_the_library() {
        let header = "#let theorem(..args) = args\n#let proof(body) = body\n\
                      #let pair(a, b) = a\n#let n = 3\n#let thm = thmbox(\"t\")";
        let s = |name| call_shape(&[header], name);
        assert_eq!(s("theorem"), shape(true, true));
        assert_eq!(s("proof"), shape(true, false));
        assert_eq!(s("pair"), shape(true, true));
        assert_eq!(s("n"), None, "not written as a function");
        assert_eq!(s("thm"), None);
        assert_eq!(s("nonsense"), None);
        assert_eq!(s("align"), shape(true, false));
        assert_eq!(s("pagebreak"), shape(false, false));
        assert_eq!(s("sym"), shape(false, false), "a module, not a function");
        // A later code block redefines `proof`; one that only sets a rule
        // does not hide it.
        let code = "#let proof(t, body) = body";
        assert_eq!(call_shape(&[header, code], "proof"), shape(true, true));
        assert_eq!(
            call_shape(&[header, "#set text(red)"], "proof"),
            shape(true, false)
        );
    }
}
