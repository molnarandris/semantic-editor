//! What a function takes, for calls typed in the editor: as a block
//! (`#lemma[…]`, `#align(center)[…]`) or in math (`sqrt(x)`, `root(3, x)`,
//! `ket(psi)`). Read from the `#let`s of the header and the code blocks, else
//! from Typst's standard library. Parsing only, no compile, so it is cheap
//! enough to run on a keystroke.

use std::sync::LazyLock;

use model::{CallShape, MathCallShape};
use typst::foundations::Value;
use typst::syntax::ast::{self, Expr, LetBindingKind, Param};
use typst::syntax::parse;
use typst::{Library, LibraryExt};

static LIBRARY: LazyLock<Library> = LazyLock::new(Library::default);

/// The parameters of a function written in Typst: positional ones, and
/// whether it has a sink (`..args`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Params {
    positional: usize,
    sink: bool,
}

/// How `sources` (the header, then the code blocks, in document order; a
/// later definition wins) define `name`: `Some(Some(params))` for a
/// function written as one, `Some(None)` for anything else bound to it
/// (`#let theorem = thmbox(…)`), `None` if they do not bind it.
fn defined(sources: &[&str], name: &str) -> Option<Option<Params>> {
    sources.iter().rev().find_map(|s| defined_in(s, name))
}

/// The shape of function `name` called as a block, by `sources`, else by
/// the standard library. `None` if unknown: bound to something that is not
/// written as a function here, imported from a package, or not defined.
pub fn call_shape(sources: &[&str], name: &str) -> Option<CallShape> {
    if let Some(found) = defined(sources, name) {
        let p = found?;
        return Some(CallShape {
            body: p.positional >= 1 || p.sink,
            title: p.positional >= 2 || p.sink,
        });
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

/// The shape of `name` called in math: a function defined by `sources`,
/// else one of Typst's math functions (`sqrt`, `cal`, `root`, `cases`) or
/// a symbol that can be called (`hat`, `floor`). One slot per positional
/// parameter (at least one); a variadic one makes room for more. `None` if
/// it is not something to call: a value, a plain symbol (`phi`), unknown,
/// or a single letter (never a call in Typst math).
pub fn math_call_shape(sources: &[&str], name: &str) -> Option<MathCallShape> {
    if name.chars().count() < 2 {
        return None;
    }
    if let Some(found) = defined(sources, name) {
        let p = found?;
        return Some(MathCallShape {
            slots: p.positional.max(1),
            variadic: p.sink,
        });
    }
    match LIBRARY.math.scope().get(name)?.read() {
        Value::Func(func) => {
            let fixed = func
                .params()
                .filter(|p| p.positional() && !p.variadic())
                .count();
            let variadic = func.params().any(|p| p.positional() && p.variadic());
            Some(MathCallShape {
                slots: (fixed + usize::from(variadic)).max(1),
                variadic,
            })
        }
        Value::Symbol(symbol) if symbol.func().is_ok() => Some(MathCallShape {
            slots: 1,
            variadic: false,
        }),
        _ => None,
    }
}

/// How a top-level `#let` in `source` binds `name` (the last one wins), as
/// in [`defined`].
fn defined_in(source: &str, name: &str) -> Option<Option<Params>> {
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
                let mut params = Params {
                    positional: 0,
                    sink: false,
                };
                for param in closure.params().children() {
                    match param {
                        Param::Pos(_) => params.positional += 1,
                        Param::Spread(_) => params.sink = true,
                        Param::Named(_) => {}
                    }
                }
                found = Some(Some(params));
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

    #[test]
    fn math_shapes() {
        let header = "#let ket(x) = x\n#let pair(a, b) = a\n#let many(..xs) = xs\n#let k = 1";
        let m = |name| math_call_shape(&[header], name).map(|s| (s.slots, s.variadic));
        assert_eq!(m("sqrt"), Some((1, false)));
        assert_eq!(m("cal"), Some((1, false)));
        assert_eq!(m("root"), Some((2, false)), "index and radicand");
        assert_eq!(m("abs"), Some((1, false)), "size is named");
        assert_eq!(m("underbrace"), Some((2, false)));
        assert_eq!(m("cases"), Some((1, true)));
        assert_eq!(m("binom"), Some((2, true)));
        assert_eq!(m("hat"), Some((1, false)), "a symbol that can be called");
        assert_eq!(m("floor"), Some((1, false)));
        assert_eq!(m("phi"), None, "a plain symbol");
        assert_eq!(m("nonsense"), None);
        assert_eq!(m("ket"), Some((1, false)));
        assert_eq!(m("pair"), Some((2, false)));
        assert_eq!(m("many"), Some((1, true)));
        assert_eq!(m("k"), None);
        assert_eq!(math_call_shape(&["#let f(x) = x"], "f"), None, "one letter");
    }

    /// `model::OPTION_CALLS` are the math functions (and callable symbols)
    /// of Typst's library with named parameters that are not content
    /// (`attach`'s are; `text` and `equation` are not called in math).
    #[test]
    fn option_calls_match_the_library() {
        let mut names = vec![];
        for (name, binding) in LIBRARY.math.scope().iter() {
            let func = match binding.read() {
                Value::Func(f) => f.clone(),
                Value::Symbol(s) => match s.func() {
                    Ok(f) => f,
                    Err(_) => continue,
                },
                _ => continue,
            };
            if ["attach", "text", "equation"].contains(&name.as_str()) {
                continue;
            }
            if func.params().any(|p| p.named() && !p.positional()) {
                names.push(name.to_string());
            }
        }
        names.sort();
        assert_eq!(names, model::OPTION_CALLS);
    }
}
