//! Code blocks: raw Typst blocks that only define things (`#let`, `#set`,
//! `#show`, `#import`, comments). Their code applies to every block after
//! them, so on the page they show as a slim marker (or, on request, as a code
//! listing) instead of their (empty) output.

use typst::syntax::{SyntaxKind, SyntaxNode, ast, parse};

use crate::serialize::quoted;

/// Whether `source` is a code block: not empty, and its top level holds only
/// definitions, rules, imports and comments.
pub fn is_code(source: &str) -> bool {
    let root = parse(source);
    let mut any = false;
    for node in root.children() {
        match node.kind() {
            SyntaxKind::Space | SyntaxKind::Parbreak | SyntaxKind::Hash | SyntaxKind::Semicolon => {
            }
            SyntaxKind::LineComment
            | SyntaxKind::BlockComment
            | SyntaxKind::LetBinding
            | SyntaxKind::SetRule
            | SyntaxKind::ShowRule
            | SyntaxKind::ModuleImport => any = true,
            _ => return false,
        }
    }
    any
}

/// The labels that `@label` references in Typst `source` point to.
pub fn refs_in_source(source: &str) -> Vec<String> {
    fn walk(node: &SyntaxNode, out: &mut Vec<String>) {
        if let Some(r) = node.cast::<ast::Ref>() {
            let label = r.target().to_string();
            if !out.contains(&label) {
                out.push(label);
            }
        }
        for child in node.children() {
            walk(child, out);
        }
    }
    let mut out = vec![];
    walk(&parse(source), &mut out);
    out
}

/// Typst code that shows a code block on the page: a marker naming what it
/// defines, or with `expanded` the source as a listing. Explicit styles, so
/// the document's own rules do not restyle it much.
pub fn code_display(source: &str, expanded: bool) -> String {
    let frame = "width: 100%, radius: 2pt, fill: rgb(\"#f4f2fa\"), \
                 stroke: (paint: rgb(\"#b8b0d8\"), thickness: 0.5pt, dash: \"dashed\")";
    if expanded {
        format!(
            "\n#block({frame}, inset: 6pt)[#text(size: 8pt, raw(block: true, lang: \"typ\", {}))]",
            quoted(source)
        )
    } else {
        format!(
            "\n#block({frame}, inset: (x: 5pt, y: 2pt))[#text(font: \"DejaVu Sans Mono\", \
             size: 7pt, fill: rgb(\"#6b6390\"), style: \"normal\", weight: \"regular\", {})]",
            quoted(&summary(source))
        )
    }
}

/// What a code block defines, e.g. `{ }  let ct, sl · set heading`.
fn summary(source: &str) -> String {
    const MAX: usize = 70;
    let root = parse(source);
    let mut lets = vec![];
    let mut rules = vec![];
    for node in root.children() {
        match node.kind() {
            SyntaxKind::LetBinding => lets.extend(let_name(node)),
            SyntaxKind::SetRule | SyntaxKind::ShowRule | SyntaxKind::ModuleImport => {
                rules.push(head(node));
            }
            _ => {}
        }
    }
    let mut parts = vec![];
    if !lets.is_empty() {
        parts.push(format!("let {}", lets.join(", ")));
    }
    parts.extend(rules);
    let mut text = format!("{{ }}  {}", parts.join(" · "));
    if parts.is_empty() {
        text.push_str("comment");
    }
    if text.chars().count() > MAX {
        text = text.chars().take(MAX - 1).collect::<String>() + "…";
    }
    text
}

/// The name a `let` binds (`ct` in `let ct(x) = …`), if it is a plain name.
fn let_name(node: &SyntaxNode) -> Option<String> {
    let target = node
        .children()
        .find(|n| matches!(n.kind(), SyntaxKind::Ident | SyntaxKind::Closure))?;
    let ident = if target.kind() == SyntaxKind::Ident {
        target
    } else {
        target.children().find(|n| n.kind() == SyntaxKind::Ident)?
    };
    Some(ident.leaf_text().to_string())
}

/// `set heading`, `show figure`, `import "@preview/cetz:0.5.3"`: the keyword
/// and its target.
fn head(node: &SyntaxNode) -> String {
    node.children()
        .filter(|n| !n.kind().is_trivia())
        .take(2)
        .map(|n| match n.kind() {
            SyntaxKind::Colon => String::new(),
            _ => n.full_text().to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Around, BridgeWorld, block_document, compile};

    #[test]
    fn code_blocks_only_define() {
        assert!(is_code("#let a = 1\n#let f(x) = x + 1"));
        assert!(is_code(
            "// helpers\n#set heading(numbering: \"A.1\")\n#show link: underline"
        ));
        assert!(is_code("#import \"@preview/cetz:0.5.3\": canvas"));
        assert!(is_code("#let a = 1; #let b = 2"));
        assert!(!is_code(""));
        assert!(!is_code("Text"));
        assert!(!is_code("#let a = 1\nText using #a"));
        assert!(!is_code("#counter(heading).update(0)"));
        // `let` inside a code block is local to it.
        assert!(!is_code("#{ let a = 1 }"));
    }

    #[test]
    fn summary_names_the_definitions() {
        assert_eq!(
            summary("#let ct(x, y: 1) = x\n#let g-r = 1.2\n#set heading(numbering: \"A.1\")"),
            "{ }  let ct, g-r · set heading"
        );
        assert_eq!(summary("// just a note"), "{ }  comment");
        assert!(summary(&"#let abcdefgh = 1\n".repeat(20)).ends_with('…'));
    }

    /// Both displays compile, with the code's own rules in force.
    #[test]
    fn displays_compile() {
        let mut world = BridgeWorld::new();
        let source = "#set text(fill: red)\n#let f(x) = \"quote \\\" and \\\\ backslash\"";
        for expanded in [false, true] {
            let around = Around {
                suffix: code_display(source, expanded),
                ..Around::default()
            };
            let (src, _) = block_document(&around, source, 300.0);
            let doc = compile(&mut world, &src).unwrap();
            assert_eq!(doc.pages().len(), 1);
            assert!(doc.pages()[0].frame.height().to_pt() > 10.0);
        }
    }
}
