//! Symbol shortcuts for math input: names typed as words (`chi`, `\\alpha`)
//! and operator pairs (`<=`, `->`).

/// Bare words that become symbols, following Typst's math names.
const WORDS: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("omicron", "ο"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Upsilon", "Υ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("oo", "∞"),
    ("infinity", "∞"),
    ("sum", "∑"),
    ("product", "∏"),
    ("integral", "∫"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("in", "∈"),
    ("times", "×"),
    ("dot", "⋅"),
    ("dots", "…"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("emptyset", "∅"),
    ("dagger", "†"),
];

/// `\name` commands (TeX names) beyond the bare words.
const COMMANDS: &[(&str, &str)] = &[
    ("epsilon", "ϵ"),
    ("varepsilon", "ε"),
    ("phi", "ϕ"),
    ("varphi", "φ"),
    ("vartheta", "ϑ"),
    ("infty", "∞"),
    ("prod", "∏"),
    ("int", "∫"),
    ("cdot", "⋅"),
    ("ldots", "…"),
    ("cdots", "⋯"),
    ("pm", "±"),
    ("mp", "∓"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("equiv", "≡"),
    ("sim", "∼"),
    ("propto", "∝"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("Rightarrow", "⇒"),
    ("Leftrightarrow", "⇔"),
    ("iff", "⇔"),
    ("mapsto", "↦"),
    ("hbar", "ℏ"),
    ("ell", "ℓ"),
    ("otimes", "⊗"),
    ("oplus", "⊕"),
    ("langle", "⟨"),
    ("rangle", "⟩"),
    ("subset", "⊂"),
    ("subseteq", "⊆"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("perp", "⊥"),
    ("parallel", "∥"),
];

/// Operator pairs: the previous symbol and the typed char.
const PAIRS: &[(&str, char, &str)] = &[
    ("<", '=', "≤"),
    (">", '=', "≥"),
    ("!", '=', "≠"),
    ("-", '>', "→"),
    ("−", '>', "→"),
    ("<", '-', "←"),
    ("←", '>', "↔"),
    ("=", '>', "⇒"),
    ("≤", '>', "⇔"),
    ("<", '<', "≪"),
    (">", '>', "≫"),
    ("+", '-', "±"),
    ("-", '+', "∓"),
    (":", '=', "≔"),
    ("∼", '~', "≈"),
    ("|", '-', "⊢"),
    ("⊢", '>', "↦"),
];

/// The symbol for a finished word, if it names one. `command` if it was
/// typed with a leading backslash.
pub fn word(name: &str, command: bool) -> Option<&'static str> {
    let find =
        |table: &[(&str, &'static str)]| table.iter().find(|(n, _)| *n == name).map(|(_, s)| *s);
    if command {
        find(COMMANDS).or_else(|| find(WORDS))
    } else {
        find(WORDS)
    }
}

/// The symbol replacing `prev` when `c` is typed right after it.
pub fn pair(prev: &str, c: char) -> Option<&'static str> {
    PAIRS
        .iter()
        .find(|(p, ch, _)| *p == prev && *ch == c)
        .map(|(_, _, s)| *s)
}

/// How a typed operator char is stored.
pub fn typed_symbol(c: char) -> String {
    match c {
        '~' => "∼".into(),
        _ => c.to_string(),
    }
}

/// Symbols that behave like letters: they can be the numerator or base
/// picked up by `/`, `^` and `_`.
pub fn is_letterlike(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '∞' | '∂' | '∇' | '∅' | 'ℏ' | 'ℓ' | '†'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(word("chi", false), Some("χ"));
        assert_eq!(word("log", false), None);
        assert_eq!(word("leq", false), None);
        assert_eq!(word("leq", true), Some("≤"));
        assert_eq!(word("alpha", true), Some("α"));
        assert_eq!(pair("<", '='), Some("≤"));
        assert_eq!(pair("≤", '='), None);
        assert!(is_letterlike("χ") && is_letterlike("∞") && !is_letterlike("+"));
    }
}
