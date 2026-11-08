//! Count the Typst constructs a document uses, to see what the editor must
//! support. Prints counts and function names only, never the text.
//!
//!   build-aux/sdk cargo run -p typst-bridge --example inventory -- FILE.typ

use std::collections::BTreeMap;

use typst::syntax::{SyntaxKind, SyntaxNode, ast, parse};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Mode {
    Markup,
    Math,
    Code,
}

#[derive(Default)]
struct Counts {
    kinds: BTreeMap<(Mode, String), usize>,
    calls: BTreeMap<(Mode, String), usize>,
    rules: BTreeMap<String, usize>,
    /// What a label follows: (kind of the previous non-space sibling).
    labelled: BTreeMap<String, usize>,
    equations: BTreeMap<&'static str, usize>,
    /// Display equations by number of lines (`\` in math).
    lines: BTreeMap<usize, usize>,
}

fn name(node: &SyntaxNode) -> String {
    let text = node.full_text();
    if text.len() > 40 {
        "<long>".into()
    } else {
        text.to_string()
    }
}

fn walk(node: &SyntaxNode, mode: Mode, c: &mut Counts) {
    let mode = match node.kind() {
        SyntaxKind::Markup => Mode::Markup,
        SyntaxKind::Math => Mode::Math,
        SyntaxKind::Code | SyntaxKind::CodeBlock => Mode::Code,
        _ => mode,
    };
    let kind = node.kind();
    if !matches!(
        kind,
        SyntaxKind::Text | SyntaxKind::Space | SyntaxKind::Parbreak | SyntaxKind::Markup
    ) {
        *c.kinds.entry((mode, format!("{kind:?}"))).or_default() += 1;
    }
    match kind {
        SyntaxKind::FuncCall | SyntaxKind::MathCall => {
            if let Some(callee) = node.children().next() {
                *c.calls.entry((mode, name(callee))).or_default() += 1;
            }
        }
        SyntaxKind::SetRule | SyntaxKind::ShowRule => {
            let target: Vec<String> = node
                .children()
                .filter(|n| !n.kind().is_trivia() && !n.kind().is_keyword())
                .take(1)
                .map(name)
                .collect();
            *c.rules
                .entry(format!("{kind:?} {}", target.join("")))
                .or_default() += 1;
        }
        SyntaxKind::Equation => {
            if let Some(eq) = node.cast::<ast::Equation>() {
                let block = eq.block();
                *c.equations
                    .entry(if block { "display" } else { "inline" })
                    .or_default() += 1;
                if block {
                    let breaks = count(node, SyntaxKind::Linebreak);
                    *c.lines.entry(breaks + 1).or_default() += 1;
                }
            }
        }
        _ => {}
    }
    let children: Vec<&SyntaxNode> = node.children().collect();
    for (i, child) in children.iter().enumerate() {
        if child.kind() == SyntaxKind::Label {
            let prev = children[..i]
                .iter()
                .rev()
                .find(|n| !n.kind().is_trivia() && n.kind() != SyntaxKind::Space);
            let what = match prev {
                Some(p) if p.kind() == SyntaxKind::FuncCall => {
                    format!("call {}", p.children().next().map(name).unwrap_or_default())
                }
                Some(p) => format!("{:?}", p.kind()),
                None => "nothing".into(),
            };
            *c.labelled.entry(what).or_default() += 1;
        }
        walk(child, mode, c);
    }
}

fn count(node: &SyntaxNode, kind: SyntaxKind) -> usize {
    usize::from(node.kind() == kind) + node.children().map(|n| count(n, kind)).sum::<usize>()
}

fn print<K: std::fmt::Debug>(title: &str, map: &BTreeMap<K, usize>) {
    println!("\n## {title}");
    let mut rows: Vec<_> = map.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in rows {
        println!("{n:6}  {k:?}");
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: inventory FILE.typ");
    let text = std::fs::read_to_string(&path).expect("read file");
    let root = parse(&text);
    let mut c = Counts::default();
    walk(&root, Mode::Markup, &mut c);
    println!("{} bytes, {} lines", text.len(), text.lines().count());
    print("Equations", &c.equations);
    print("Display equations by line count", &c.lines);
    print("Function calls (mode, callee)", &c.calls);
    print("Set/show rules", &c.rules);
    print("Labels follow", &c.labelled);
    print("Syntax kinds (mode, kind)", &c.kinds);
}
