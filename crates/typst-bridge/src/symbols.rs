//! Typst's symbol names (`times`, `times.o`, `arrow.r.long`) resolved
//! through the standard library, for math input in the editor.

use std::sync::LazyLock;

use typst::foundations::Value;
use typst::{Library, LibraryExt};

static LIBRARY: LazyLock<Library> = LazyLock::new(Library::default);

/// The text of the math symbol `name` with its `.modifiers`, as Typst would
/// show it, or `None` if `name` is not a symbol (an operator like `log`, a
/// function like `sqrt`, or unknown).
pub fn resolve_symbol(name: &str) -> Option<String> {
    let mut parts = name.split('.');
    let base = parts.next().filter(|b| !b.is_empty())?;
    let Value::Symbol(symbol) = LIBRARY.math.scope().get(base)?.read() else {
        return None;
    };
    let mut symbol = symbol.clone();
    for modifier in parts {
        if modifier.is_empty() {
            return None;
        }
        symbol = symbol.modified((), modifier).ok()?;
    }
    Some(symbol.get().to_string())
}

/// What can be typed as a name in a formula, sorted: every symbol of
/// Typst's math module with each of its variants (`arrow.r.long`, shown as
/// its char), its functions (`sqrt`) and operators (`lim`). Deprecated
/// variants are left out.
pub fn math_names() -> &'static [(String, String)] {
    static NAMES: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
        let mut names = vec![];
        for (name, binding) in LIBRARY.math.scope().iter() {
            match binding.read() {
                Value::Symbol(symbol) => {
                    for (modifiers, glyph, deprecated) in symbol.variants() {
                        if deprecated.is_some() {
                            continue;
                        }
                        let full = match modifiers.as_str() {
                            "" => name.to_string(),
                            m => format!("{name}.{m}"),
                        };
                        names.push((full, glyph.to_string()));
                    }
                }
                Value::Func(_) => names.push((name.to_string(), "function".into())),
                Value::Content(_) => names.push((name.to_string(), "operator".into())),
                _ => {}
            }
        }
        names.sort();
        names.dedup_by(|a, b| a.0 == b.0);
        names
    });
    &NAMES
}

/// Whether `name` is defined by Typst's math module but not globally
/// (`dif`, `cal`): code in a formula must write it `math.name`.
pub fn math_only(name: &str) -> bool {
    LIBRARY.math.scope().get(name).is_some() && LIBRARY.global.scope().get(name).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_modifiers() {
        let r = |n| resolve_symbol(n);
        assert_eq!(r("times").as_deref(), Some("×"));
        assert_eq!(r("times.o").as_deref(), Some("⊗"));
        assert_eq!(r("arrow.r.long").as_deref(), Some("⟶"));
        assert_eq!(r("chi").as_deref(), Some("χ"));
        assert_eq!(r("eq.not").as_deref(), Some("≠"));
        assert_eq!(r("times.nonsense"), None);
        assert_eq!(r("times."), None);
        assert_eq!(r("log"), None, "operator, not a symbol");
        assert_eq!(r("sqrt"), None, "function");
        assert_eq!(r("x"), None);
    }

    #[test]
    fn names_to_complete() {
        let names = math_names();
        let find = |n: &str| names.iter().find(|(m, _)| m == n).map(|(_, d)| d.as_str());
        assert_eq!(find("alpha"), Some("α"));
        assert_eq!(find("arrow.r.long"), Some("⟶"));
        assert_eq!(find("sqrt"), Some("function"));
        assert_eq!(find("lim"), Some("operator"));
        assert!(names.len() > 500, "{}", names.len());
    }
}
