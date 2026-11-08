use super::*;
use crate::tree::MathKind;

/// "ab $x$ cd" / "ef" / display "y" / "gh"
fn sample() -> Editor {
    let mut d = Document::empty();
    let (t1, x, t2) = (d.text("ab "), d.ident("x"), d.text(" cd"));
    let m = d.math(vec![x]);
    let p1 = d.paragraph(vec![t1, m, t2]);
    let t = d.text("ef");
    let p2 = d.paragraph(vec![t]);
    let y = d.ident("y");
    let disp = d.display(vec![y]);
    let t = d.text("gh");
    let p3 = d.paragraph(vec![t]);
    d.blocks = vec![p1, p2, disp, p3];
    Editor::new(d)
}

fn texts(e: &Editor) -> Vec<String> {
    e.doc()
        .blocks
        .iter()
        .map(|b| match &b.kind {
            Block::Paragraph(v) | Block::Heading { inlines: v, .. } => v
                .iter()
                .map(|i| match &i.kind {
                    Inline::Text(s) => s.clone(),
                    Inline::Math(_) => "$".into(),
                })
                .collect(),
            Block::DisplayMath(_) => "$$".into(),
            Block::RawTypst(s) | Block::Diagram(s) => s.clone(),
        })
        .collect()
}

fn caret_at(e: &mut Editor, block: usize, inline: usize, offset: usize) {
    let id = e.doc().blocks[block].id;
    e.set_caret(CursorPath {
        block: id,
        path: vec![Step::Index(inline)],
        offset,
    });
}

#[test]
fn typing_groups_words_for_undo() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 2);
    for c in "xy zw".chars() {
        assert!(e.insert_text(&c.to_string()));
    }
    assert_eq!(texts(&e)[1], "efxy zw");
    assert_eq!(e.caret().offset, 7);
    e.undo();
    assert_eq!(texts(&e)[1], "efxy ");
    assert_eq!(e.caret().offset, 5);
    e.undo();
    assert_eq!(texts(&e)[1], "ef");
    assert_eq!(e.caret().offset, 2);
    e.redo();
    e.redo();
    assert_eq!(texts(&e)[1], "efxy zw");
    // A caret move starts a new group.
    e.move_left();
    e.insert_text("Q");
    e.undo();
    assert_eq!(texts(&e)[1], "efxy zw");
}

#[test]
fn extra_spaces_are_not_inserted() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 2);
    e.insert_text(" ");
    assert!(!e.insert_text(" "), "second space ignored");
    e.insert_text("x  \t y");
    assert_eq!(texts(&e)[1], "ef x y");
    // Before an existing space, a typed space is dropped too.
    caret_at(&mut e, 1, 0, 2);
    assert!(!e.insert_text(" "));
    // No space at the start of a paragraph.
    caret_at(&mut e, 1, 0, 0);
    assert!(!e.insert_text(" "));
    assert_eq!(e.caret().offset, 0);
}

#[test]
fn deleting_never_leaves_double_spaces() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 2);
    e.insert_text(" b c");
    caret_at(&mut e, 1, 0, 4); // after "b"
    e.backspace();
    assert_eq!(texts(&e)[1], "ef c");
    assert_eq!(e.caret().offset, 2);
    caret_at(&mut e, 1, 0, 0);
    e.delete();
    e.delete();
    assert_eq!(texts(&e)[1], "c", "no leading space");
    // Removing a formula between two spaces keeps one.
    caret_at(&mut e, 0, 2, 0);
    e.backspace();
    e.backspace();
    assert_eq!(texts(&e)[0], "ab cd");
}

#[test]
fn backspace_and_delete_chars() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 1);
    assert!(e.backspace());
    assert_eq!(texts(&e)[1], "f");
    assert!(!e.doc().blocks.is_empty());
    assert!(e.delete());
    assert_eq!(texts(&e)[1], "");
    e.undo();
    assert_eq!(texts(&e)[1], "ef");
}

#[test]
fn backspace_removes_formula_and_joins_text() {
    let mut e = sample();
    caret_at(&mut e, 0, 2, 0);
    assert!(e.backspace());
    assert_eq!(texts(&e)[0], "ab $ cd", "first press selects");
    assert!(e.selected().is_some());
    assert!(e.backspace());
    assert_eq!(texts(&e)[0], "ab cd");
    assert_eq!(e.caret().path, vec![Step::Index(0)]);
    assert_eq!(e.caret().offset, 3);
    e.undo();
    assert_eq!(texts(&e)[0], "ab $ cd");
    assert!(e.doc().check_invariants().is_ok());
}

#[test]
fn enter_splits_and_backspace_joins() {
    let mut e = sample();
    caret_at(&mut e, 0, 0, 1);
    assert!(e.enter());
    assert_eq!(texts(&e)[..2], ["a", "b $ cd"]);
    assert_eq!(e.caret().block, e.doc().blocks[1].id);
    assert_eq!(e.caret().offset, 0);
    assert!(e.backspace());
    assert_eq!(texts(&e)[..2], ["ab $ cd", "ef"]);
    assert_eq!(e.caret().offset, 1);
    // Delete at the end of a paragraph joins the next one.
    let n = e.doc().blocks[0].kind.inlines().unwrap().len();
    caret_at(&mut e, 0, n - 1, 3);
    assert!(e.delete());
    assert_eq!(texts(&e)[0], "ab $ cdef");
    e.undo();
    e.undo();
    e.undo();
    assert_eq!(texts(&e)[..2], ["ab $ cd", "ef"]);
    assert!(e.doc().check_invariants().is_ok());
}

#[test]
fn equals_space_makes_headings() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 0);
    e.insert_text("== ");
    assert!(matches!(
        e.doc().blocks[1].kind,
        Block::Heading { level: 2, .. }
    ));
    assert_eq!(texts(&e)[1], "ef");
    assert_eq!(e.caret().offset, 0);
    // Backspace at the start: back to a paragraph, then join.
    e.backspace();
    assert!(matches!(e.doc().blocks[1].kind, Block::Paragraph(_)));
    e.backspace();
    assert_eq!(texts(&e)[0], "ab $ cdef");
    while e.undo() {}
    assert_eq!(texts(&e)[1], "ef");
    // `=` elsewhere is just text; shortcuts set the level.
    caret_at(&mut e, 1, 0, 1);
    e.insert_text("= ");
    assert_eq!(texts(&e)[1], "e= f");
    assert!(e.set_heading(Some(3)));
    assert!(matches!(
        e.doc().blocks[1].kind,
        Block::Heading { level: 3, .. }
    ));
    assert!(e.set_heading(None));
    assert!(!e.set_heading(None));
}

#[test]
fn header_edits_are_one_undo_step_and_keep_the_caret() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 2);
    let caret = e.caret().clone();
    assert!(e.set_header("#set text(size: 12pt)"));
    assert!(e.set_header("#set text(size: 12pt)\n#set heading(numbering: \"1.\")"));
    assert!(!e.set_header("#set text(size: 12pt)\n#set heading(numbering: \"1.\")"));
    assert_eq!(e.caret(), &caret);
    e.undo();
    assert_eq!(e.doc().header, "");
    e.redo();
    assert!(e.doc().header.ends_with("numbering: \"1.\")"));
}

#[test]
fn diagram_blocks() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 2);
    e.backspace();
    e.backspace();
    // "ef" is now empty: the diagram takes its place.
    assert!(e.insert_block(NewBlock::Diagram));
    let id = e.caret().block;
    assert_eq!(e.doc().blocks.len(), 4);
    assert_eq!(e.block_source(id), Some(DIAGRAM_TEMPLATE));
    // Source edits from the pane are one undo step.
    e.set_block_source(id, "circle((0, 0))");
    e.set_block_source(id, "circle((0, 0))\nrect((0, 0), (1, 1))");
    e.undo();
    assert_eq!(e.block_source(id), Some(DIAGRAM_TEMPLATE));
    e.redo();
    // Escape goes to the next paragraph (here the display formula is
    // next, so a new paragraph is created).
    assert!(e.escape());
    assert_eq!(e.doc().blocks.len(), 5);
    assert!(matches!(e.doc().blocks[2].kind, Block::Paragraph(_)));
    // Backspace selects the block, then deletes it.
    e.set_caret(cursor(id, vec![], 0));
    assert!(e.backspace());
    assert_eq!(e.selected_ids(), vec![id]);
    assert!(e.backspace());
    assert!(e.doc().block(id).is_none());
    assert_eq!(e.caret().block, e.doc().blocks[0].id);
    while e.undo() {}
    assert_eq!(texts(&e)[1], "ef");
}

#[test]
fn enter_at_end_of_heading_starts_paragraph() {
    let mut d = Document::empty();
    let t = d.text("Title");
    let h = d.heading(1, vec![t]);
    d.blocks = vec![h];
    let mut e = Editor::new(d);
    caret_at(&mut e, 0, 0, 5);
    e.enter();
    assert!(matches!(e.doc().blocks[1].kind, Block::Paragraph(_)));
    caret_at(&mut e, 0, 0, 2);
    e.enter();
    assert!(matches!(e.doc().blocks[1].kind, Block::Heading { .. }));
}

#[test]
fn arrows_enter_formulas_and_cross_blocks() {
    let mut e = sample();
    caret_at(&mut e, 0, 0, 2);
    let mut seen = vec![];
    for _ in 0..5 {
        e.move_right();
        seen.push((e.caret().path.clone(), e.caret().offset));
    }
    let i = Step::Index;
    assert_eq!(
        seen,
        [
            (vec![i(0)], 3),
            (vec![i(1)], 0), // inside the formula, before x
            (vec![i(1)], 1),
            (vec![i(2)], 0), // after the formula
            (vec![i(2)], 1),
        ]
    );
    for _ in 0..4 {
        e.move_left();
    }
    assert_eq!((e.caret().path.clone(), e.caret().offset), (vec![i(0)], 3));
    // Across blocks: end of "ef" → display row → "gh".
    caret_at(&mut e, 1, 0, 2);
    e.move_right();
    assert_eq!(e.caret().block, e.doc().blocks[2].id);
    assert_eq!(e.caret().offset, 0);
    e.move_right();
    e.move_right();
    assert_eq!(e.caret().block, e.doc().blocks[3].id);
    e.move_left();
    assert_eq!(e.caret().block, e.doc().blocks[2].id);
    assert_eq!(e.caret().offset, 1);
}

#[test]
fn empty_paragraph_before_formula_is_removed() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 2);
    e.backspace();
    e.backspace();
    // Now "ef" is empty; Delete removes the empty block before the formula.
    assert!(e.delete());
    assert_eq!(texts(&e), ["ab $ cd", "$$", "gh"]);
    // Backspace at the start of "gh" goes into the formula block.
    caret_at(&mut e, 2, 0, 0);
    assert!(e.backspace());
    assert_eq!(e.caret().block, e.doc().blocks[1].id);
}

/// A compact rendering of a row for assertions.
fn show(row: &crate::tree::MathRow) -> String {
    row.nodes
        .iter()
        .map(|n| match &n.kind {
            MathKind::Sym(s) | MathKind::Ident(s) | MathKind::Num(s) => s.clone(),
            MathKind::Slot => "□".into(),
            MathKind::Frac { num, den } => format!("frac({}, {})", show(num), show(den)),
            MathKind::Scripts { base, sup, sub } => format!(
                "{}{}{}",
                show(base),
                sup.as_ref()
                    .map_or(String::new(), |r| format!("^({})", show(r))),
                sub.as_ref()
                    .map_or(String::new(), |r| format!("_({})", show(r))),
            ),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The display formula of block `b`.
fn display_of(e: &Editor, b: usize) -> String {
    match &e.doc().blocks[b].kind {
        Block::DisplayMath(row) => show(row),
        _ => panic!("not display math"),
    }
}

fn inline_of(e: &Editor, b: usize, i: usize) -> String {
    match &e.doc().blocks[b].kind.inlines().unwrap()[i].kind {
        Inline::Math(row) => show(row),
        _ => panic!("not a formula"),
    }
}

fn empty_display() -> Editor {
    let mut d = Document::empty();
    let b = d.display(vec![]);
    d.blocks = vec![b];
    Editor::new(d)
}

fn labels(e: &Editor) -> Vec<&'static str> {
    e.doc()
        .breadcrumb(e.caret())
        .iter()
        .map(|c| c.label)
        .collect()
}

/// The acceptance sequence of the math milestone, key by key.
#[test]
fn types_the_entropy_bound_structurally() {
    let mut e = empty_display();
    e.insert_text("S <= (log chi)");
    assert_eq!(display_of(&e, 0), "S ≤ ( log χ )");
    e.insert_text("/");
    assert_eq!(display_of(&e, 0), "S ≤ frac(log χ, )");
    assert_eq!(labels(&e), ["display math", "fraction", "denominator"]);
    e.insert_text("(2");
    assert_eq!(labels(&e), ["display math", "fraction", "denominator"]);
    e.insert_text(")");
    assert_eq!(labels(&e), ["display math"]);
    assert_eq!(e.caret().offset, 3, "just after the fraction");
    e.insert_text(" + c");
    assert_eq!(display_of(&e, 0), "S ≤ frac(log χ, 2) + c");
    // One undo step per structural action; all of it undoes cleanly.
    while e.undo() {}
    assert_eq!(display_of(&e, 0), "");
}

#[test]
fn same_formula_with_slots_and_arrows() {
    let mut e = empty_display();
    e.insert_text("S<=/");
    assert_eq!(labels(&e), ["display math", "fraction", "numerator"]);
    e.insert_text("log chi");
    let down = e.vertical_target(true).unwrap();
    e.set_caret(down);
    e.insert_text("2");
    e.move_right(); // leaves the fraction
    e.insert_text("+c");
    assert_eq!(display_of(&e, 0), "S ≤ frac(log χ, 2) + c");
}

#[test]
fn dollar_makes_inline_and_display_formulas() {
    let mut e = sample();
    caret_at(&mut e, 1, 0, 1);
    e.insert_text("$x^2");
    assert_eq!(texts(&e)[1], "e$f");
    assert_eq!(inline_of(&e, 1, 1), "x^(2)");
    e.insert_text("$");
    assert_eq!(e.caret().path, vec![Step::Index(2)], "in the text after");
    assert_eq!(e.caret().offset, 0);
    e.insert_text(" g");
    assert_eq!(texts(&e)[1], "e$ gf");

    let mut e = sample();
    caret_at(&mut e, 1, 0, 1);
    e.insert_text("$$");
    assert_eq!(texts(&e)[1..4], ["e", "$$", "f"]);
    e.insert_text("a");
    assert_eq!(display_of(&e, 2), "a");
}

#[test]
fn scripts_and_slot_removal() {
    let mut e = empty_display();
    e.insert_text("x^2");
    assert_eq!(display_of(&e, 0), "x^(2)");
    e.escape();
    e.insert_text("_");
    assert_eq!(display_of(&e, 0), "x^(2)_()");
    e.insert_text("i");
    assert_eq!(display_of(&e, 0), "x^(2)_(i)");
    e.backspace(); // deletes i
    e.backspace(); // empty subscript slot: removed, caret at end of base
    assert_eq!(display_of(&e, 0), "x^(2)");
    assert_eq!(labels(&e), ["display math", "scripts", "base"]);
    e.escape();
    e.insert_text("/");
    assert_eq!(display_of(&e, 0), "frac(x^(2), )");
    e.backspace(); // empty denominator: unwrap to the numerator
    assert_eq!(display_of(&e, 0), "x^(2)");
    assert_eq!(e.caret().path, Vec::<Step>::new());
    assert_eq!(e.caret().offset, 1);
}

#[test]
fn backspace_selects_structures_first() {
    let mut e = empty_display();
    e.insert_text("a+(b)/c");
    e.escape();
    assert_eq!(display_of(&e, 0), "a + frac(b, c)");
    assert!(e.backspace());
    assert_eq!(display_of(&e, 0), "a + frac(b, c)");
    assert_eq!(e.selected_ids().len(), 1);
    assert!(e.backspace());
    assert_eq!(display_of(&e, 0), "a +");
    e.undo();
    assert_eq!(display_of(&e, 0), "a + frac(b, c)");
}

#[test]
fn words_symbols_and_numbers() {
    let mut e = empty_display();
    e.insert_text("\\alpha + beta2.50 -> sin x != \\leq ");
    assert_eq!(display_of(&e, 0), "α + β 2.50 → sin x ≠ ≤");
    e.insert_text("lo");
    e.backspace();
    e.insert_text("n ");
    assert_eq!(display_of(&e, 0), "α + β 2.50 → sin x ≠ ≤ ln");
}

/// A stand-in for the Typst resolver.
fn fake_resolver(name: &str) -> Option<String> {
    match name {
        "times" => Some("×".into()),
        "times.o" => Some("⊗".into()),
        "arrow" | "arrow.r" => Some("→".into()),
        "arrow.r.long" => Some("⟶".into()),
        "chi" => Some("χ".into()),
        _ => None,
    }
}

#[test]
fn dotted_symbol_names() {
    let mut e = empty_display();
    e.set_symbol_resolver(fake_resolver);
    e.insert_text("a times.o b arrow.r.long c times b");
    assert_eq!(display_of(&e, 0), "a ⊗ b ⟶ c × b");
    // Not symbols: the dot stays a dot.
    let mut e = empty_display();
    e.set_symbol_resolver(fake_resolver);
    e.insert_text("x. times.q chi. ");
    assert_eq!(display_of(&e, 0), "x . × . q χ .");
    e.insert_text("\\times.o ");
    assert_eq!(display_of(&e, 0), "x . × . q χ . ⊗");
    while e.undo() {}
    assert_eq!(display_of(&e, 0), "");
}

#[test]
fn tab_visits_empty_slots_and_escape_leaves() {
    let mut e = empty_display();
    e.insert_text("/");
    e.escape();
    e.insert_text("=/");
    e.escape();
    assert_eq!(display_of(&e, 0), "frac(, ) = frac(, )");
    let start = e.doc().block_start(0);
    e.set_caret(start);
    let mut visits = vec![];
    while e.next_slot(false) {
        visits.push(labels(&e).last().copied().unwrap());
    }
    assert_eq!(
        visits,
        ["numerator", "denominator", "numerator", "denominator"]
    );
    e.next_slot(true);
    assert_eq!(labels(&e).last(), Some(&"numerator"));
    e.escape();
    assert_eq!(e.caret().offset, 3, "after the second fraction");
    assert!(e.escape(), "leaving the display formula");
    assert!(matches!(e.doc().blocks[1].kind, Block::Paragraph(_)));
}

#[test]
fn breadcrumb_selects_structures() {
    let mut e = empty_display();
    e.insert_text("a/b");
    let crumbs = e.doc().breadcrumb(e.caret());
    assert!(e.select_crumb(crumbs[1].path_len)); // the fraction
    assert_eq!(e.selected_ids().len(), 1);
    e.backspace();
    assert_eq!(display_of(&e, 0), "");
    let mut e = sample();
    caret_at(&mut e, 0, 1, 1);
    assert_eq!(e.context_ids().len(), 1);
    assert!(e.select_crumb(1));
    assert!(matches!(
        e.selected(),
        Some(Selected::Formula { inline: 1, .. })
    ));
}
