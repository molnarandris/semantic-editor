use super::*;
use crate::tree::{ListKind, MathKind};

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

/// The top-level blocks as short strings; an environment as
/// `#name[block|block]`, with a title `#name(title)[…]`.
fn texts(e: &Editor) -> Vec<String> {
    e.doc().blocks.iter().map(block_text).collect()
}

fn inline_text(v: &[InlineNode]) -> String {
    v.iter()
        .map(|i| match &i.kind {
            Inline::Text(s) => s.clone(),
            Inline::Math(_) => "$".into(),
            Inline::Call { name, args, body } => match name.as_str() {
                "strong" => format!("*{}*", inline_text(body)),
                "emph" => format!("_{}_", inline_text(body)),
                _ if args.is_empty() => format!("#{name}[{}]", inline_text(body)),
                _ => format!("#{name}({args})[{}]", inline_text(body)),
            },
        })
        .collect()
}

fn block_text(b: &BlockNode) -> String {
    match &b.kind {
        Block::Paragraph(v) | Block::Heading { inlines: v, .. } => inline_text(v),
        Block::DisplayMath(_) => "$$".into(),
        Block::RawTypst(s) | Block::Diagram(s) => s.clone(),
        Block::Env {
            name,
            args,
            title,
            body,
        } => {
            let inner: Vec<String> = body.iter().map(block_text).collect();
            let name = match args.is_empty() {
                true => name.clone(),
                false => format!("{name}{{{args}}}"),
            };
            let title = match title.is_empty() {
                true => String::new(),
                false => format!("({})", inline_text(title)),
            };
            format!("#{name}{title}[{}]", inner.join("|"))
        }
        Block::Item { kind, term, body } => {
            let inner: Vec<String> = body.iter().map(block_text).collect();
            let marker = match kind {
                ListKind::Bullet => "-".into(),
                ListKind::Numbered => "+".into(),
                ListKind::Term => format!("/{}:", inline_text(term)),
            };
            format!("{marker}[{}]", inner.join("|"))
        }
    }
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

/// `#lemma` + space makes an environment; typing, Enter, arrows, Escape and
/// Backspace work inside it, and at its edges (just before and after it,
/// outside it).
#[test]
fn environments() {
    let mut d = Document::empty();
    let t = d.text("Before");
    d.blocks = vec![d.paragraph(vec![t])];
    let mut e = Editor::new(d);
    caret_at(&mut e, 0, 0, 6);
    e.enter();
    e.insert_text("#lemma");
    e.insert_text(" ");
    assert_eq!(texts(&e), ["Before", "#lemma[]"]);
    e.insert_text("First.");
    e.enter();
    e.insert_text("Second.");
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]"]);
    e.doc().check_invariants().unwrap();
    let env = e.doc().blocks[1].id;
    let (before, after) = (cursor(env, vec![], 0), cursor(env, vec![], 1));
    let body = |e: &Editor| e.doc().blocks[1].kind.body().unwrap().clone();

    // ← from the start of the body: just before it, then the paragraph
    // before; → back the same way.
    e.set_caret(e.doc().start_of(&body(&e)[0]));
    assert!(e.move_left());
    assert_eq!(e.caret(), &before);
    assert!(e.move_left());
    assert_eq!(e.caret(), &e.doc().end_of(&e.doc().blocks[0]));
    assert!(e.move_right());
    assert_eq!(e.caret(), &before);
    assert!(e.move_right());
    assert_eq!(e.caret(), &e.doc().start_of(&body(&e)[0]));

    // → at the end of the body: just after it.
    e.set_caret(e.doc().end_of(&body(&e)[1]));
    assert!(e.move_right());
    assert_eq!(e.caret(), &after);

    // Escape in the body: just after it, no new paragraph.
    e.set_caret(e.doc().start_of(&body(&e)[0]));
    assert!(e.escape());
    assert_eq!(e.caret(), &after);
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]"]);

    // Typing just after it starts a paragraph after it.
    e.insert_text("After");
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]", "After"]);

    // Enter on an empty last paragraph of the body moves it out.
    e.set_caret(e.doc().end_of(&body(&e)[1]));
    e.enter();
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.|]", "After"]);
    e.enter();
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]", "", "After"]);
    e.backspace();
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]", "After"]);
    assert_eq!(e.caret(), &after);

    // Typing just before it starts a paragraph before it.
    e.set_caret(before.clone());
    e.insert_text("x");
    assert_eq!(
        texts(&e),
        ["Before", "x", "#lemma[First.|Second.]", "After"]
    );
    e.undo();
    e.undo();
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]", "After"]);

    // Backspace at the start of the paragraph after it: just after it; the
    // next press selects it, the one after deletes it.
    e.set_caret(e.doc().start_of(&e.doc().blocks[2]));
    e.backspace();
    assert_eq!(e.caret(), &after);
    e.backspace();
    assert_eq!(e.selected(), Some(&Selected::Block { block: env }));
    e.backspace();
    assert_eq!(texts(&e), ["Before", "After"]);
    e.undo();

    // Backspace at the start of the body marks it; the next press unwraps
    // it, keeping its text and the caret; one undo brings it back.
    let first = e.doc().start_of(&body(&e)[0]);
    e.set_caret(first.clone());
    e.backspace();
    assert_eq!(e.selected(), Some(&Selected::Unwrap { env }));
    assert_eq!(e.env_focus(), Some((env, true)));
    assert!(e.env_marked());
    e.backspace();
    assert_eq!(texts(&e), ["Before", "First.", "Second.", "After"]);
    assert_eq!(e.caret(), &first);
    e.doc().check_invariants().unwrap();
    e.undo();
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]", "After"]);
    e.set_caret(first);
    e.backspace();
    e.escape();
    assert_eq!(e.selected(), None);

    // An empty environment goes with one Backspace.
    e.set_caret(e.doc().end_of(&e.doc().blocks[2]));
    e.enter();
    e.insert_text("#proof ");
    assert_eq!(texts(&e)[3], "#proof[]");
    e.backspace();
    assert_eq!(texts(&e), ["Before", "#lemma[First.|Second.]", "After"]);
    e.doc().check_invariants().unwrap();

    // Not an identifier: stays text.
    e.insert_text(" #1 ");
    assert_eq!(texts(&e)[2], "After #1 ");
}

/// An environment's optional title: `#name(` starts one, `)` or Enter goes
/// on to the body, an empty title goes when the caret leaves it, and keys at
/// its ends never reach the blocks around.
#[test]
fn environment_titles() {
    let mut d = Document::empty();
    let t = d.text("Before");
    d.blocks = vec![d.paragraph(vec![t])];
    let mut e = Editor::new(d);
    caret_at(&mut e, 0, 0, 6);
    e.enter();
    e.insert_text("#lemma(Gap (spectral)) Text.");
    assert_eq!(texts(&e), ["Before", "#lemma(Gap (spectral))[Text.]"]);
    e.doc().check_invariants().unwrap();
    let env = e.doc().blocks[1].id;
    let title_start = cursor(env, vec![Step::Index(0)], 0);

    // ← from the body: through the title, then just before the environment.
    e.set_caret(e.doc().start_of(&e.doc().blocks[1].kind.body().unwrap()[0]));
    assert!(e.move_left());
    assert_eq!(e.caret(), &cursor(env, vec![Step::Index(0)], 14));
    e.set_caret(title_start.clone());
    assert!(e.move_left());
    assert_eq!(e.caret(), &cursor(env, vec![], 0));

    // Keys at the ends of the title stay in it; Esc leaves the environment.
    e.set_caret(title_start.clone());
    assert!(!e.backspace());
    e.set_caret(cursor(env, vec![Step::Index(0)], 14));
    assert!(!e.delete());
    assert_eq!(texts(&e), ["Before", "#lemma(Gap (spectral))[Text.]"]);
    assert!(e.escape());
    assert_eq!(e.caret(), &cursor(env, vec![], 1));

    // A formula in the title; `$$` in it does not make a display formula.
    e.set_caret(cursor(env, vec![Step::Index(0)], 3));
    e.insert_text("$$x");
    assert_eq!(texts(&e)[1], "#lemma(Gap$x (spectral))[Text.]");
    e.doc().check_invariants().unwrap();
    while e.undo() {}

    // Adding a title to an environment that has none, and leaving it empty.
    let mut d = Document::empty();
    let t = d.text("Text.");
    let p = d.paragraph(vec![t]);
    d.blocks = vec![d.env("lemma", vec![p])];
    let mut e = Editor::new(d);
    let env = e.doc().blocks[0].id;
    assert!(e.edit_title(env));
    assert_eq!(texts(&e), ["#lemma()[Text.]"]);
    assert_eq!(e.env_focus(), Some((env, true)));
    assert!(e.move_right());
    assert_eq!(texts(&e), ["#lemma[Text.]"], "an empty title goes");
    assert_eq!(
        e.caret(),
        &e.doc().start_of(&e.doc().blocks[0].kind.body().unwrap()[0])
    );
    e.doc().check_invariants().unwrap();

    // Typed, then edited again (the caret goes to its end); Enter → body.
    e.edit_title(env);
    e.insert_text("Key");
    e.set_caret(e.doc().doc_end());
    assert_eq!(texts(&e), ["#lemma(Key)[Text.]"]);
    e.edit_title(env);
    assert_eq!(e.caret(), &cursor(env, vec![Step::Index(0)], 3));
    e.enter();
    assert_eq!(
        e.caret(),
        &e.doc().start_of(&e.doc().blocks[0].kind.body().unwrap()[0])
    );

    // Backspace in an empty title removes it.
    e.edit_title(env);
    for _ in 0..3 {
        e.backspace();
    }
    assert_eq!(texts(&e), ["#lemma()[Text.]"]);
    e.backspace();
    assert_eq!(texts(&e), ["#lemma[Text.]"]);
    e.doc().check_invariants().unwrap();
}

/// What the fake bridge knows: `lemma` takes a title, `proof` and `align`
/// only a body, `pagebreak` nothing.
fn fake_calls(_: &[&str], name: &str) -> Option<CallShape> {
    let (body, title) = match name {
        "lemma" => (true, true),
        "proof" | "align" => (true, false),
        "pagebreak" => (false, false),
        _ => return None,
    };
    Some(CallShape { body, title })
}

/// Block calls by what the function takes: `(` gives a title only to a
/// function that takes one, else starts source arguments; a function that
/// takes no content stays text; an unknown one may take a title.
#[test]
fn block_calls_by_function() {
    let fresh = || {
        let mut e = Editor::new(Document::new());
        e.set_call_resolver(fake_calls);
        e
    };
    let mut e = fresh();
    e.insert_text("#lemma(Key) Text");
    assert_eq!(texts(&e), ["#lemma(Key)[Text]"]);
    let mut e = fresh();
    e.insert_text("#unknown(Key) Text");
    assert_eq!(texts(&e), ["#unknown(Key)[Text]"]);

    let mut e = fresh();
    e.insert_text("#align(center + horizon) Text");
    assert_eq!(texts(&e), ["#align{center + horizon}[Text]"]);
    let mut e = fresh();
    e.insert_text("#proof[Text");
    assert_eq!(texts(&e), ["#proof[Text]"]);
    e.doc().check_invariants().unwrap();
    // No title for proof: a click on its heading does nothing.
    let env = e.doc().blocks[0].id;
    assert!(!e.edit_title(env));
    assert_eq!(texts(&e), ["#proof[Text]"]);

    let mut e = fresh();
    e.insert_text("#pagebreak x");
    assert_eq!(texts(&e), ["#pagebreak x"]);
    // Unbalanced arguments stay text.
    let mut e = fresh();
    e.insert_text("#align(a)) x");
    assert_eq!(texts(&e), ["#align(a)) x"]);
}

/// Lists: `- ` makes an item, Enter the next one, Tab/Shift+Tab nest and
/// un-nest, Enter on an empty item moves it out a level and then out of the
/// list, Backspace at an item's start lets its text continue the item before.
#[test]
fn lists() {
    let mut e = Editor::new(Document::new());
    let check = |e: &Editor| e.doc().check_invariants().unwrap();
    e.insert_text("- a");
    assert_eq!(texts(&e), ["-[a]"]);
    e.enter();
    e.insert_text("b");
    e.enter();
    assert_eq!(texts(&e), ["-[a]", "-[b]", "-[]"]);
    assert!(e.next_slot(false), "Tab nests the empty item in b");
    assert_eq!(texts(&e), ["-[a]", "-[b|-[]]"]);
    e.insert_text("c");
    e.enter();
    assert_eq!(texts(&e), ["-[a]", "-[b|-[c]|-[]]"]);
    check(&e);
    e.enter();
    assert_eq!(texts(&e), ["-[a]", "-[b|-[c]]", "-[]"], "out a level");
    e.enter();
    assert_eq!(texts(&e), ["-[a]", "-[b|-[c]]", ""], "out of the list");
    e.insert_text("after");
    check(&e);

    // Backspace at the start of b: its text (and c) continue item a; again:
    // the paragraphs join.
    let b = e.doc().blocks[1].kind.body().unwrap()[0].clone();
    e.set_caret(e.doc().start_of(&b));
    e.backspace();
    assert_eq!(texts(&e), ["-[a|b|-[c]]", "after"]);
    e.backspace();
    assert_eq!(texts(&e), ["-[ab|-[c]]", "after"]);
    check(&e);

    // Shift+Tab in c: out after a. Enter in the middle of "ab" splits it.
    let c = e.doc().blocks[0].kind.body().unwrap()[1].clone();
    e.set_caret(e.doc().end_of(&c));
    assert!(e.next_slot(true));
    assert_eq!(texts(&e), ["-[ab]", "-[c]", "after"]);
    let ab = e.doc().blocks[0].kind.body().unwrap()[0].id;
    e.set_caret(cursor(ab, vec![Step::Index(0)], 1));
    e.enter();
    assert_eq!(texts(&e), ["-[a]", "-[b]", "-[c]", "after"]);
    check(&e);

    // Enter in an item with a nested list after the text: the nested list
    // goes with the new item, so the order stays.
    e.set_caret(e.doc().end_of(&e.doc().blocks[1]));
    e.next_slot(false);
    assert_eq!(texts(&e), ["-[a|-[b]]", "-[c]", "after"]);
    e.set_caret(cursor(
        e.doc().blocks[0].kind.body().unwrap()[0].id,
        vec![Step::Index(0)],
        1,
    ));
    e.enter();
    assert_eq!(texts(&e), ["-[a]", "-[|-[b]]", "-[c]", "after"]);
    check(&e);
    while e.undo() {}
    assert_eq!(texts(&e), [""]);

    // Numbered and term items; `:` ends the term.
    e.insert_text("+ one");
    e.enter();
    e.insert_text("two");
    assert_eq!(texts(&e), ["+[one]", "+[two]"]);
    // Shift+Tab on a numbered item nested in a bullet: it becomes a bullet.
    let mut e = Editor::new(Document::new());
    e.insert_text("- a");
    e.enter();
    e.enter();
    e.insert_text("+ b");
    assert!(e.next_slot(false));
    assert_eq!(texts(&e), ["-[a|+[b]]"]);
    assert!(e.next_slot(true));
    assert_eq!(texts(&e), ["-[a]", "-[b]"]);
    check(&e);

    let mut e = Editor::new(Document::new());
    e.insert_text("/ Term: desc");
    assert_eq!(texts(&e), ["/Term:[desc]"]);
    e.enter();
    assert_eq!(texts(&e), ["/Term:[desc]", "/:[]"]);
    assert!(e.term_item().is_some(), "the caret is in the new term");
    e.backspace();
    assert_eq!(
        texts(&e),
        ["/Term:[desc|]"],
        "an empty term drops the marker"
    );
    check(&e);
}

/// Enter on a raw/diagram block adds a paragraph: after it at its end (also
/// after the last block), before it at its start.
#[test]
fn enter_on_source_blocks_adds_paragraphs() {
    let mut d = Document::empty();
    let t = d.text("Text");
    d.blocks = vec![d.paragraph(vec![t]), d.diagram("circle((0, 0))")];
    let mut e = Editor::new(d);
    let id = e.doc().blocks[1].id;
    let end = "circle((0, 0))".chars().count();
    e.set_caret(cursor(id, vec![], end));
    assert!(e.enter());
    assert_eq!(e.doc().blocks.len(), 3);
    assert!(matches!(e.doc().blocks[2].kind, Block::Paragraph(_)));
    assert_eq!(e.caret().block, e.doc().blocks[2].id);

    e.set_caret(cursor(id, vec![], 0));
    assert!(e.enter());
    assert_eq!(e.doc().blocks.len(), 4);
    assert!(matches!(e.doc().blocks[1].kind, Block::Paragraph(_)));
    assert_eq!(e.doc().blocks[2].id, id);
    assert_eq!(e.caret(), &cursor(id, vec![], 0));
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

/// "Insert Diagram" in a formula inserts an inline one, a code atom: its
/// source edits are one undo step, Ctrl+Enter finds it next to the caret,
/// Backspace selects it, then deletes it.
#[test]
fn inline_diagrams() {
    let mut e = empty_display();
    e.insert_text("x");
    assert!(e.insert_block(NewBlock::Diagram));
    e.insert_text("y");
    assert_eq!(display_of(&e, 0), "x #diagram y");
    assert_eq!(e.doc().blocks.len(), 1, "no block was added");
    let Block::DisplayMath(row) = &e.doc().blocks[0].kind else {
        unreachable!()
    };
    let id = row.nodes[1].id;
    assert_eq!(e.source_target(), None, "the caret is after y");
    e.move_left();
    assert_eq!(e.source_target(), Some(id), "the atom before the caret");
    e.move_left();
    assert_eq!(e.source_target(), Some(id), "the atom after the caret");
    assert_eq!(e.block_source(id), Some(INLINE_DIAGRAM_TEMPLATE));
    let caret = e.caret().clone();
    assert!(e.set_block_source(id, "box(a)"));
    assert!(e.set_block_source(id, "box(ab)"));
    assert_eq!(e.caret(), &caret, "the caret stays");
    assert_eq!(e.doc().top_block(id), Some(e.doc().blocks[0].id));
    e.undo();
    assert_eq!(e.block_source(id), Some(INLINE_DIAGRAM_TEMPLATE));
    e.redo();
    assert_eq!(e.block_source(id), Some("box(ab)"));

    // Delete (caret before it) selects it; Backspace after it too.
    assert!(e.delete());
    assert_eq!(e.selected_ids(), vec![id]);
    assert_eq!(e.source_target(), Some(id), "the selected atom");
    e.escape();
    e.move_right();
    assert!(e.backspace());
    assert_eq!(e.selected_ids(), vec![id]);
    assert!(e.backspace());
    assert_eq!(display_of(&e, 0), "x y");
    e.doc().check_invariants().unwrap();
}

/// What the fake bridge knows in math: `sqrt` (one slot), `root` (two),
/// `cases` (variadic), `hat` (a symbol that can be called).
fn fake_math_calls(_: &[&str], name: &str) -> Option<MathCallShape> {
    let (slots, variadic) = match name {
        "sqrt" | "hat" => (1, false),
        "root" => (2, false),
        "cases" => (1, true),
        _ => return None,
    };
    Some(MathCallShape { slots, variadic })
}

fn math_calls_editor() -> Editor {
    let mut e = empty_display();
    e.set_symbol_resolver(|n| match n {
        "hat" => Some("^".into()),
        "phi" => Some("φ".into()),
        _ => None,
    });
    e.set_math_call_resolver(fake_math_calls);
    e
}

/// Function calls in math: the name then space (if it is not a symbol) or
/// `(` makes the call with its slots, caret in the first; `)` or Esc leave
/// it, `,` goes to the next argument (or adds one to `cases`), Backspace in
/// empty arguments removes them.
#[test]
fn math_calls() {
    let mut e = math_calls_editor();
    e.insert_text("sqrt x");
    assert_eq!(display_of(&e, 0), "sqrt(x)");
    assert_eq!(labels(&e).last(), Some(&"argument"));
    e.escape();
    e.insert_text("+root(3,y)z");
    assert_eq!(display_of(&e, 0), "sqrt(x) + root(3, y) z");
    e.doc().check_invariants().unwrap();

    // `hat` is a symbol with space, a call with `(`; `phi(` stays a symbol.
    let mut e = math_calls_editor();
    e.insert_text("hat hat(a)phi(b)");
    assert_eq!(display_of(&e, 0), "^ hat(a) φ ( b )");

    // `cases`: each comma adds an argument; Backspace in an empty extra one
    // removes it, in the only (empty) one removes the call.
    let mut e = math_calls_editor();
    e.insert_text("cases(a,b,");
    assert_eq!(display_of(&e, 0), "cases(a, b, )");
    e.backspace();
    assert_eq!(display_of(&e, 0), "cases(a, b)");
    assert_eq!(e.caret().offset, 1, "at the end of b");
    e.doc().check_invariants().unwrap();
    let mut e = math_calls_editor();
    e.insert_text("sqrt ");
    assert_eq!(display_of(&e, 0), "sqrt()");
    e.backspace();
    assert_eq!(display_of(&e, 0), "");
    // A comma in sqrt is a comma.
    let mut e = math_calls_editor();
    e.insert_text("sqrt(a,b)");
    assert_eq!(display_of(&e, 0), "sqrt(a , b)");
}

/// Atoms: `'` adds primes, `"…"` is text (spaces and all, Backspace takes
/// its chars back), `&` an alignment point, `\` then space a line break.
#[test]
fn math_atoms() {
    let mut e = math_calls_editor();
    e.insert_text("f''(x)");
    assert_eq!(display_of(&e, 0), "f '' ( x )");
    e.backspace();
    e.backspace();
    e.backspace();
    e.backspace();
    assert_eq!(display_of(&e, 0), "f '", "Backspace takes one prime");

    let mut e = math_calls_editor();
    e.insert_text("x \"if a, $b\" y");
    assert_eq!(display_of(&e, 0), "x \"if a, $b\" y");
    e.backspace();
    e.backspace();
    e.backspace();
    assert_eq!(
        display_of(&e, 0),
        "x \"if a, \"",
        "y, then chars of the text"
    );
    e.insert_text("z");
    assert_eq!(
        display_of(&e, 0),
        "x \"if a, \" z",
        "not typing it any more"
    );
    let mut e = math_calls_editor();
    e.insert_text("\"");
    assert_eq!(display_of(&e, 0), "\"\"");
    e.backspace();
    assert_eq!(display_of(&e, 0), "", "an empty text goes");

    let mut e = math_calls_editor();
    e.insert_text("a&=b\\ &=c");
    assert_eq!(display_of(&e, 0), "a & = b \\ & = c");
    e.doc().check_invariants().unwrap();
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
            MathKind::Code(_) => "#code".into(),
            MathKind::Diagram(_) => "#diagram".into(),
            MathKind::Primes(n) => "'".repeat(*n),
            MathKind::Text(t) => format!("\"{t}\""),
            MathKind::Align => "&".into(),
            MathKind::Linebreak => "\\".into(),
            MathKind::Call { name, args } => format!(
                "{name}({})",
                args.iter().map(show).collect::<Vec<_>>().join(", ")
            ),
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

    // Large operators take scripts too (limits on `sum`).
    let mut e = empty_display();
    e.set_symbol_resolver(|n| (n == "sum").then(|| "∑".into()));
    e.insert_text("sum_i");
    assert_eq!(display_of(&e, 0), "∑_(i)");
    let mut e = empty_display();
    e.insert_text("+_");
    assert_eq!(display_of(&e, 0), "+ _()", "an operator does not");
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
    let start = e.doc().doc_start();
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
        Some(Selected::Inline { path, .. }) if path == &[Step::Index(1)]
    ));
}

#[test]
fn bold_and_italic_as_in_typst() {
    let mut e = Editor::default();
    e.insert_text("a *bold* and _it_ end");
    assert_eq!(texts(&e), ["a *bold* and _it_ end"]);
    // In a word, `*` and `_` are plain text (Typst reads them so too);
    // `\*` is a literal star.
    let mut e = Editor::default();
    e.insert_text("snake_case a*b \\* \\_");
    assert_eq!(texts(&e), ["snake_case a*b * _"]);
    // Nested, with a formula inside; `$` leaves the formula, `*` the call.
    let mut e = Editor::default();
    e.insert_text("*x _y $a$ z_*.");
    assert_eq!(texts(&e), ["*x _y $ z_*."]);
    assert_eq!(e.caret().path, vec![Step::Index(2)]);
    e.doc().check_invariants().unwrap();
    while e.undo() {}
    assert_eq!(texts(&e), [""]);
}

#[test]
fn inline_calls_typed() {
    let mut e = Editor::default();
    e.insert_text("see #cl[closure] and #link(\"https://x.org\")[here] or #todo fix]!");
    assert_eq!(
        texts(&e),
        ["see #cl[closure] and #link(\"https://x.org\")[here] or #todo[fix]!"]
    );
    // `#name` + space as the whole paragraph is still a block call.
    let mut e = Editor::default();
    e.insert_text("#lemma x");
    assert_eq!(texts(&e), ["#lemma[x]"]);
    // Escape leaves the call; an empty call left goes.
    let mut e = Editor::default();
    e.insert_text("a *b");
    assert!(e.escape());
    e.insert_text(" c **d");
    assert_eq!(texts(&e), ["a *b* c d"]);
}

#[test]
fn backspace_around_inline_calls() {
    let mut e = Editor::default();
    e.insert_text("a *b* c");
    caret_at(&mut e, 0, 2, 0);
    assert!(e.backspace());
    assert!(e.selected().is_some(), "first press selects the call");
    assert!(e.backspace());
    assert_eq!(texts(&e), ["a c"]);
    // In an empty call: it goes. At the start of a full one: ←.
    let mut e = Editor::default();
    e.insert_text("a *");
    assert!(e.backspace());
    assert_eq!(texts(&e), ["a "]);
    assert_eq!(e.caret().path, vec![Step::Index(0)]);
    let mut e = Editor::default();
    e.insert_text("a *b");
    e.move_left();
    assert!(e.backspace());
    assert_eq!(texts(&e), ["a *b*"]);
    assert_eq!(
        (e.caret().path.clone(), e.caret().offset),
        (vec![Step::Index(0)], 2)
    );
}

#[test]
fn enter_splits_inline_calls() {
    let mut e = Editor::default();
    e.insert_text("a *bo _x_ld* c");
    // Caret in "ld" after "l": inside the bold.
    let caret = e.caret().clone();
    let path = vec![Step::Index(1), Step::Body, Step::Index(2)];
    e.set_caret(CursorPath {
        path: path.clone(),
        offset: 1,
        ..caret
    });
    assert!(e.enter());
    assert_eq!(texts(&e), ["a *bo _x_l*", "*d* c"]);
    assert_eq!(
        e.caret().path,
        vec![Step::Index(1), Step::Body, Step::Index(0)]
    );
    e.insert_text("!");
    assert_eq!(texts(&e), ["a *bo _x_l*", "*!d* c"]);
    // At the end of the bold, nothing of it continues.
    let mut e = Editor::default();
    e.insert_text("a *b");
    assert!(e.enter());
    assert_eq!(texts(&e), ["a *b*", ""]);
    e.doc().check_invariants().unwrap();
    while e.undo() {}
    assert_eq!(texts(&e), [""]);
}

#[test]
fn arrows_go_through_inline_calls() {
    let mut e = Editor::default();
    e.insert_text("a *b* c");
    caret_at(&mut e, 0, 0, 2);
    assert!(e.move_right());
    assert_eq!(
        e.caret().path,
        vec![Step::Index(1), Step::Body, Step::Index(0)]
    );
    let crumbs: Vec<_> = e
        .doc()
        .breadcrumb(e.caret())
        .iter()
        .map(|c| c.label)
        .collect();
    assert_eq!(crumbs, ["paragraph", "bold"]);
    assert!(e.move_right());
    assert!(e.move_right());
    assert_eq!(
        (e.caret().path.clone(), e.caret().offset),
        (vec![Step::Index(2)], 0)
    );
}

#[test]
fn an_empty_call_left_goes() {
    for right in [false, true] {
        let mut e = Editor::default();
        e.insert_text("a b");
        // An empty bold typed in the middle, then the caret moves away.
        caret_at(&mut e, 0, 0, 2);
        e.insert_text("*");
        assert_eq!(texts(&e), ["a **b"]);
        let moved = if right { e.move_right() } else { e.move_left() };
        assert!(moved);
        assert_eq!(texts(&e), ["a b"]);
        assert_eq!(
            (e.caret().path.clone(), e.caret().offset),
            (vec![Step::Index(0)], 2)
        );
        e.doc().check_invariants().unwrap();
        e.undo();
        assert_eq!(texts(&e), ["a **b"]);
    }
}

#[test]
fn call_focus_is_the_innermost_call() {
    let mut e = Editor::default();
    e.insert_text("a *b _c $x");
    let p = |v: &[Step]| Some(v.to_vec());
    assert_eq!(
        e.call_focus(),
        p(&[Step::Index(1), Step::Body, Step::Index(1)])
    );
    e.insert_text("$");
    e.escape();
    assert_eq!(e.call_focus(), p(&[Step::Index(1)]));
    e.escape();
    assert_eq!(e.call_focus(), None);
}
