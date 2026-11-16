use super::*;
use crate::remarks::Anchor;
use crate::tree::{ListKind, MathKind, TitleFields};

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
            Inline::Raw(s) => format!("`{s}`"),
            Inline::Code(s) => format!("{{#{s}}}"),
            Inline::Ref(s) => format!("@{{{s}}}"),
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
        Block::Caption(v) => format!("cap({})", inline_text(v)),
        Block::Bibliography(_) => "bib".into(),
        Block::Title(_) => "title".into(),
        Block::DisplayMath(_) => "$$".into(),
        Block::RawTypst(s) | Block::Diagram(s) => s.clone(),
        Block::Image { path, opts } => format!("img({path};{opts})"),
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
    // Before an existing space, a typed space is not stored, but goes in
    // with the text typed next: a new word between two.
    caret_at(&mut e, 1, 0, 2);
    assert!(!e.insert_text(" "));
    assert_eq!(texts(&e)[1], "ef x y");
    e.insert_text("new");
    assert_eq!(texts(&e)[1], "ef new x y");
    assert_eq!(e.caret().offset, 6);
    // Moving away drops it, even when coming back.
    caret_at(&mut e, 1, 0, 6);
    e.insert_text(" ");
    e.move_left();
    e.move_right();
    e.insert_text("Q");
    assert_eq!(texts(&e)[1], "ef newQ x y");
    e.undo();
    // A formula typed there gets the space too.
    caret_at(&mut e, 1, 0, 2);
    e.insert_text(" ");
    e.insert_text("$");
    assert_eq!(texts(&e)[1], "ef $ new x y");
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

    // → at the end of the body: just after it (past its label).
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
/// takes no content is a code piece; one defined nowhere is code too (it
/// shows failing), except with `[`.
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
    assert_eq!(texts(&e), ["{#unknown(Key)} Text"]);
    let mut e = fresh();
    e.insert_text("#unknown x");
    assert_eq!(texts(&e), ["{#unknown} x"]);
    let mut e = fresh();
    e.insert_text("#unknown[x");
    assert_eq!(texts(&e), ["#unknown[x]"]);

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
    assert_eq!(texts(&e), ["{#pagebreak} x"], "a code piece, no block call");
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
        "sqrt" | "hat" | "lr" | "abs" => (1, false),
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

/// The options tab of a math call: ↑ from its arguments reaches it (the
/// widget picks the char by x); ←/→ skip it and stay in it; every char is
/// kept; ↓, Enter or Esc go into the first argument. Calls that take no
/// options have none.
#[test]
fn math_call_options() {
    let up = |e: &mut Editor| {
        let t = e.vertical_target(false).unwrap();
        e.set_caret(t);
    };
    let mut e = math_calls_editor();
    e.insert_text("lr ");
    assert_eq!(display_of(&e, 0), "lr()");
    up(&mut e);
    assert_eq!(e.caret().path.last(), Some(&Step::Opts));
    assert_eq!(labels(&e).last(), Some(&"options"));
    e.insert_text("size: 80%");
    assert_eq!(display_of(&e, 0), "lr[size: 80%]()");
    e.backspace();
    e.insert_text("%");
    assert!(!e.move_right(), "→ stays in the tab");
    assert!(e.move_left());
    assert_eq!(e.caret().offset, 8);
    let down = e.vertical_target(true).unwrap();
    assert_eq!(down.path.last(), Some(&Step::Arg(0)));
    assert!(e.enter());
    assert_eq!(e.caret().path.last(), Some(&Step::Arg(0)));
    e.insert_text("(x)");
    assert_eq!(display_of(&e, 0), "lr[size: 80%](( x ))");
    e.doc().check_invariants().unwrap();
    // ← from the start of the argument and → from before the call skip the
    // tab; ↑ from a fraction's denominator goes to the numerator first.
    let mut e = math_calls_editor();
    e.insert_text("abs(1/y");
    e.move_left();
    assert_eq!(e.caret().path.last(), Some(&Step::Den));
    up(&mut e);
    assert_eq!(e.caret().path.last(), Some(&Step::Num));
    up(&mut e);
    assert_eq!(e.caret().path.last(), Some(&Step::Opts));
    e.insert_text("size: 2em");
    e.escape();
    assert_eq!(e.caret().path.last(), Some(&Step::Arg(0)));
    assert_eq!(display_of(&e, 0), "abs[size: 2em](frac(1, y))");
    assert!(e.move_left());
    assert!(e.caret().path.is_empty(), "before the call");
    assert!(e.move_right());
    assert_eq!(e.caret().path.last(), Some(&Step::Arg(0)));
    // Undo takes the options back.
    while e.undo() {}
    assert_eq!(display_of(&e, 0), "");
    // `sqrt` takes no options: ↑ finds no tab.
    let mut e = math_calls_editor();
    e.insert_text("sqrt ");
    assert!(e.vertical_target(false).is_none());
}

/// `cases`: Enter starts a new case after the caret's one (also from deeper
/// in it), ↑/↓ go to the case above/below, `)` still leaves.
#[test]
fn cases_by_lines() {
    let mut e = math_calls_editor();
    e.insert_text("cases(a");
    assert!(e.enter());
    e.insert_text("1/b");
    assert!(e.enter(), "from the denominator: a new case after it");
    e.insert_text("c");
    assert_eq!(display_of(&e, 0), "cases(a, frac(1, b), c)");
    let arg = |e: &Editor| e.caret().path[1];
    assert_eq!(arg(&e), Step::Arg(2));
    assert!(
        e.vertical_target(true).is_none(),
        "below the last case: lines"
    );
    let up = e.vertical_target(false).unwrap();
    assert_eq!(up.path[1], Step::Arg(1));
    e.set_caret(up);
    e.set_caret(e.vertical_target(false).unwrap());
    assert_eq!(arg(&e), Step::Arg(0));
    // Enter in the first case inserts after it.
    e.set_caret(CursorPath {
        offset: 1,
        ..e.caret().clone()
    });
    assert!(e.enter());
    e.insert_text("x)");
    assert_eq!(display_of(&e, 0), "cases(a, x, frac(1, b), c)");
    assert!(e.caret().path.is_empty(), "`)` left the call");
    e.doc().check_invariants().unwrap();
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
    // Across blocks: end of "ef" → display row → "gh" (not its label).
    caret_at(&mut e, 1, 0, 2);
    e.move_right();
    assert_eq!(e.caret().block, e.doc().blocks[2].id);
    assert_eq!(e.caret().offset, 0);
    e.move_right();
    e.move_right();
    assert_eq!(e.caret().block, e.doc().blocks[3].id);
    e.move_left();
    assert_eq!(e.caret().block, e.doc().blocks[2].id);
    assert_eq!(e.caret().path, vec![]);
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

/// The display formula of block `b`, `␣` marking the spaces kept.
fn spaced_display(e: &Editor, b: usize) -> String {
    let Block::DisplayMath(row) = &e.doc().blocks[b].kind else {
        panic!("not display math");
    };
    row.nodes
        .iter()
        .map(|n| {
            let one = crate::tree::MathRow {
                id: row.id,
                nodes: vec![n.clone()],
            };
            format!("{}{}", if n.space { "␣" } else { "" }, show(&one))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A space typed in a formula is kept where it shows, next to a bar or
/// text, as Typst does: `3 |x| - 1` is not `3|x|-1`.
#[test]
fn spaces_next_to_bars_and_text() {
    let mut e = math_calls_editor();
    e.insert_text("3 |x| - 1");
    assert_eq!(
        spaced_display(&e, 0),
        "3 ␣| x | ␣- 1",
        "no ⊢ across the space"
    );
    // Elsewhere a space is ignored; next to text it is kept.
    let mut e = math_calls_editor();
    e.insert_text("a b \"if\" c\"t\"");
    assert_eq!(spaced_display(&e, 0), "a b ␣\"if\" ␣c \"t\"");
    // Typed into an existing gap; Backspace takes it away first.
    let mut e = math_calls_editor();
    e.insert_text("x|y");
    assert_eq!(spaced_display(&e, 0), "x | y");
    e.move_left();
    e.insert_text(" ");
    assert_eq!(spaced_display(&e, 0), "x | ␣y");
    assert!(!e.insert_text(" "), "one space");
    e.backspace();
    assert_eq!(spaced_display(&e, 0), "x | y");
    e.backspace();
    assert_eq!(spaced_display(&e, 0), "x y", "then the bar");
    e.undo();
    assert_eq!(
        spaced_display(&e, 0),
        "x | ␣y",
        "one undo step brings both back"
    );
    // Backspace right after typing a space forgets it.
    let mut e = math_calls_editor();
    e.insert_text("a ");
    e.backspace();
    e.insert_text("|");
    assert_eq!(spaced_display(&e, 0), "a |");
    e.doc().check_invariants().unwrap();
}

/// A compact rendering of a row for assertions.
fn show(row: &crate::tree::MathRow) -> String {
    row.nodes
        .iter()
        .map(|n| match &n.kind {
            MathKind::Sym(s) | MathKind::Ident(s) | MathKind::Num(s) => s.clone(),
            MathKind::Slot => "□".into(),
            MathKind::Code(s) => format!("#{s}"),
            MathKind::Diagram(_) => "#diagram".into(),
            MathKind::Primes(n) => "'".repeat(*n),
            MathKind::Text(t) => format!("\"{t}\""),
            MathKind::Align => "&".into(),
            MathKind::Linebreak => "\\".into(),
            MathKind::Call { name, opts, args } => format!(
                "{name}{}({})",
                if opts.is_empty() {
                    String::new()
                } else {
                    format!("[{opts}]")
                },
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

/// `#code ` in text makes a code piece: with arguments or dots, or a
/// function taking no content; the space stays, the caret after it; one
/// undo makes it text again. The caret in it opens it; typing edits it.
#[test]
fn hash_code_in_text() {
    let fresh = || {
        let mut e = Editor::new(Document::new());
        e.set_call_resolver(fake_calls);
        e
    };
    let mut e = fresh();
    e.insert_text("a #h(1em) b #sym.dagger c");
    assert_eq!(texts(&e), ["a {#h(1em)} b {#sym.dagger} c"]);
    e.doc().check_invariants().unwrap();
    assert_eq!(e.open_code(), None);

    let mut e = fresh();
    e.insert_text("a #pagebreak ");
    assert_eq!(texts(&e), ["a {#pagebreak} "]);
    e.undo();
    assert_eq!(texts(&e), ["a #pagebreak"]);

    // A function taking content still makes a call (one defined nowhere
    // only with `[`: a typo is code, shown failing); unbalanced stays text.
    let mut e = fresh();
    e.insert_text("a #proof x");
    assert_eq!(texts(&e), ["a #proof[x]"]);
    let mut e = fresh();
    e.insert_text("a #sym.dager x #cl x #cl[y");
    assert_eq!(texts(&e), ["a {#sym.dager} x {#cl} x #cl[y]"]);
    let mut e = fresh();
    e.insert_text("a #f(x b");
    assert_eq!(texts(&e), ["a #f(x b"]);

    // Into it from the right: open, edited as typed.
    let mut e = fresh();
    e.insert_text("a #h(1em) b");
    e.move_left();
    e.move_left();
    assert_eq!(e.open_code(), None, "before the space");
    e.move_left();
    assert!(e.open_code().is_some());
    for _ in 0..4 {
        e.backspace();
    }
    e.insert_text("2em)");
    assert_eq!(texts(&e), ["a {#h(2em)} b"]);
}

/// Code in a formula is edited in place: ←/→ walk through its source
/// (open while the caret is in it), typing keeps every char, Backspace
/// takes chars, Esc leaves it just after; emptied, it goes. Undo works.
#[test]
fn code_in_formulas_is_edited_in_place() {
    let mut d = Document::empty();
    let (x, code, y) = (d.ident("x"), d.code("sym.dager"), d.ident("y"));
    let code_id = code.id;
    d.blocks = vec![d.display(vec![x, code, y])];
    let mut e = Editor::new(d);
    let id = e.doc().blocks[0].id;
    e.set_caret(cursor(id, vec![], 1));
    assert_eq!(e.open_code(), None);
    e.move_right();
    assert_eq!(e.caret(), &cursor(id, vec![Step::Index(1), Step::Code], 0));
    assert_eq!(e.open_code(), Some(code_id));
    assert_eq!(e.source_target(), Some(code_id));
    for _ in 0..7 {
        e.move_right();
    }
    e.insert_text("g");
    assert_eq!(display_of(&e, 0), "x #sym.dagger y");
    e.move_right();
    e.move_right();
    e.insert_text(", (");
    assert_eq!(display_of(&e, 0), "x #sym.dagger, ( y", "kept as typed");
    e.backspace();
    e.backspace();
    e.backspace();
    e.escape();
    assert_eq!(e.caret(), &cursor(id, vec![], 2));
    assert_eq!(e.open_code(), None);
    assert_eq!(display_of(&e, 0), "x #sym.dagger y");
    while e.undo() {}
    assert_eq!(display_of(&e, 0), "x #sym.dager y");

    // Emptied and left, it goes.
    e.set_caret(cursor(id, vec![Step::Index(1), Step::Code], 9));
    for _ in 0..9 {
        e.backspace();
    }
    assert_eq!(display_of(&e, 0), "x # y");
    e.escape();
    assert_eq!(display_of(&e, 0), "x y");
    assert_eq!(e.caret(), &cursor(id, vec![], 1));
    e.doc().check_invariants().unwrap();
}

/// `$` at the very start of a formula: inline ⇄ display, the caret
/// staying there; going inline joins the paragraphs around it.
#[test]
fn dollar_at_the_start_toggles_display() {
    let mut e = sample();
    caret_at(&mut e, 0, 1, 0);
    e.insert_text("$");
    assert_eq!(texts(&e)[..3], ["ab ", "$$", "cd"]);
    assert_eq!(display_of(&e, 1), "x");
    assert_eq!((e.caret().path.clone(), e.caret().offset), (vec![], 0));
    e.insert_text("$");
    assert_eq!(texts(&e)[..2], ["ab $ cd", "ef"]);
    assert_eq!(inline_of(&e, 0, 1), "x");
    assert_eq!(e.caret().path, vec![Step::Index(1)]);
    assert_eq!(e.caret().offset, 0);
    e.undo();
    assert_eq!(texts(&e)[..3], ["ab ", "$$", "cd"]);
    e.undo();
    assert_eq!(texts(&e)[..2], ["ab $ cd", "ef"]);

    // A labelled display formula between paragraphs: inline, unlabelled.
    let mut d = sample().doc().clone();
    d.blocks[2].label = "eq:y".into();
    let disp = d.blocks[2].id;
    let mut e = Editor::new(d);
    e.set_caret(CursorPath {
        block: disp,
        path: vec![],
        offset: 0,
    });
    e.insert_text("$");
    assert_eq!(texts(&e)[1..], ["ef $ gh"]);
    assert_eq!(inline_of(&e, 1, 1), "y");
    assert!(e.doc().labels().is_empty());

    // Not inside it: `$` leaves the formula as before.
    let mut e = sample();
    caret_at(&mut e, 0, 1, 1);
    e.insert_text("$");
    assert_eq!(texts(&e)[0], "ab $ cd");
    assert_eq!(e.caret().path, vec![Step::Index(2)]);
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

/// A picture: ←/→ pass it (never into its options tab); ↑ beside it enters
/// the tab, where its options are typed; ↓ or Esc leave to just after it.
#[test]
fn picture_options() {
    let mut d = Document::empty();
    let picture = d.image("images/a.png");
    let id = picture.id;
    let caption = d.caption(vec![]);
    d.blocks = vec![d.env("figure", vec![picture, caption])];
    let mut e = Editor::new(d);
    let before = CursorPath {
        block: id,
        path: vec![],
        offset: 0,
    };
    e.set_caret(before.clone());
    assert_eq!(e.tab_focus(), Some(vec![]));
    while e.move_right() {
        assert_ne!(e.caret().path, [Step::Opts]);
    }
    e.set_caret(before);
    let up = e.vertical_target(false).unwrap();
    assert_eq!(up.path, [Step::Opts]);
    e.set_caret(up);
    e.insert_text("width: 50%");
    assert_eq!(texts(&e), ["#figure[img(images/a.png;width: 50%)|cap()]"]);
    assert!(!e.move_right(), "the tab keeps the caret");
    assert!(e.escape());
    assert_eq!((e.caret().block, e.caret().path.clone()), (id, vec![]));
    assert_eq!(e.caret().offset, "images/a.png".len());
    assert_eq!(e.source_target(), None, "no source to edit");
    while e.undo() {}
    assert_eq!(texts(&e), ["#figure[img(images/a.png;)|cap()]"]);
}

/// A read-only editor refuses every change; the caret still moves and
/// selects.
#[test]
fn read_only() {
    let mut e = Editor::default();
    e.insert_text("ab $x/y$");
    e.set_read_only(true);
    let before = texts(&e);
    e.insert_text("more");
    e.backspace();
    e.enter();
    assert!(!e.undo());
    assert!(!e.insert_picture("a.png", vec![1], false));
    assert!(!e.set_header("#set text(red)"));
    e.paste(Fragment::Plain("pasted".into()));
    assert_eq!(texts(&e), before);
    assert!(e.doc().files.is_empty());
    assert!(e.move_left());
    e.set_read_only(false);
    assert!(e.undo());
}

/// Insert Picture: a figure in place of an empty paragraph, the file in the
/// project under a free name, the caret in the caption.
#[test]
fn inserting_pictures() {
    let mut e = Editor::default();
    assert!(e.insert_picture("a.png", vec![1], false));
    assert_eq!(texts(&e), ["#figure[img(images/a.png;)|cap()]"]);
    assert!(e.caption_of_caret().is_some());
    e.insert_text("A.");
    assert!(e.insert_picture("a.png", vec![2], false));
    assert_eq!(
        texts(&e),
        [
            "#figure[img(images/a.png;)|cap(A.)]",
            "#figure[img(images/a-2.png;)|cap()]"
        ]
    );
    assert_eq!(e.doc().files.get("images/a-2.png"), Some(&[2u8][..]));
    while e.undo() {}
    assert_eq!(texts(&e), [""]);
    assert!(e.doc().files.is_empty(), "undo takes the files away too");
}

/// Scratch files: a picture in `scratch/`; keeping it moves the file to
/// `images/` and the picture follows (one undo step); only unused files
/// can be deleted.
#[test]
fn scratch_files() {
    let mut e = Editor::default();
    assert!(e.insert_picture("scan.png", vec![7], true));
    assert_eq!(texts(&e), ["#figure[img(scratch/scan.png;)|cap()]"]);
    assert!(!e.delete_file("scratch/scan.png"), "in use");
    assert!(e.keep_file("scratch/scan.png"));
    assert_eq!(texts(&e), ["#figure[img(images/scan.png;)|cap()]"]);
    assert_eq!(e.doc().files.get("images/scan.png"), Some(&[7u8][..]));
    assert!(e.doc().files.get("scratch/scan.png").is_none());
    assert!(e.undo());
    assert_eq!(texts(&e), ["#figure[img(scratch/scan.png;)|cap()]"]);
    assert!(e.doc().files.get("images/scan.png").is_none());
    assert!(e.redo());
    assert!(!e.keep_file("images/scan.png"), "not a scratch file");
    // An unused file goes, and undo brings it back.
    let mut d = Document::new();
    d.files.insert("scratch/note.png", vec![8]);
    let mut e = Editor::new(d);
    assert!(!e.file_used("scratch/note.png"));
    assert!(e.delete_file("scratch/note.png"));
    assert!(e.doc().files.is_empty());
    assert!(e.undo());
    assert_eq!(e.doc().files.get("scratch/note.png"), Some(&[8u8][..]));
}

/// ←/→ pass a footnote's marker in one step; its text comes after the rest
/// of the paragraph, where it is shown.
#[test]
fn footnotes_after_their_paragraph() {
    let mut e = Editor::default();
    e.insert_text("ab#footnote[n] c#footnote[m]");
    e.enter();
    e.insert_text("x");
    let at = |e: &Editor| (e.caret().path.clone(), e.caret().offset);
    let (i, body) = (Step::Index, Step::Body);
    e.set_caret(e.doc().start_of(&e.doc().blocks[0]));
    let mut seen = vec![at(&e)];
    while e.move_right() && e.caret().block == e.doc().blocks[0].id {
        seen.push(at(&e));
    }
    assert_eq!(
        seen,
        [
            (vec![i(0)], 0),
            (vec![i(0)], 1),
            (vec![i(0)], 2),
            (vec![i(2)], 0),
            (vec![i(2)], 1),
            (vec![i(2)], 2),
            (vec![i(4)], 0),
            (vec![i(1), body, i(0)], 0),
            (vec![i(1), body, i(0)], 1),
            (vec![i(3), body, i(0)], 0),
            (vec![i(3), body, i(0)], 1),
        ]
    );
    // ← from the next paragraph: into the last footnote's end.
    assert!(e.move_left());
    assert_eq!(at(&e), (vec![i(3), body, i(0)], 1));
}

/// In a theorem the footnote's text is shown under the whole theorem, so
/// ←/→ reach it after the theorem's later blocks and its end, not right
/// after the footnote's paragraph.
#[test]
fn footnotes_after_their_theorem() {
    let mut d = Document::empty();
    let a = d.text("a");
    let n = d.text("n");
    let note = InlineNode {
        id: d.fresh_id(),
        kind: Inline::Call {
            name: "footnote".into(),
            args: String::new(),
            body: vec![n],
        },
    };
    let p = d.paragraph(vec![a, note]);
    let x = d.sym("x");
    let eq = d.display(vec![x]);
    d.blocks = vec![d.env("theorem", vec![p, eq])];
    let e = Editor::new(d);
    let env = &e.doc().blocks[0];
    let body = env.kind.body().unwrap();
    let (p, eq) = (body[0].id, body[1].id);
    let got: Vec<_> = e
        .doc()
        .positions(env)
        .iter()
        .map(|c| (c.block, c.path.clone(), c.offset))
        .collect();
    let (i, b) = (Step::Index, Step::Body);
    assert_eq!(
        got,
        [
            (env.id, vec![], 0),
            (p, vec![i(0)], 0),
            (p, vec![i(0)], 1),
            (p, vec![i(2)], 0),
            (eq, vec![], 0),
            (eq, vec![], 1),
            (env.id, vec![], 1),
            (p, vec![i(1), b, i(0)], 0),
            (p, vec![i(1), b, i(0)], 1),
        ]
    );
}

/// Backspace at the start of an inline call's body marks the call (Esc
/// cancels); the next one replaces it by its body, joined with the text
/// around it.
#[test]
fn inline_calls_unwrap() {
    let body_start = |e: &mut Editor| {
        let block = e.doc().blocks[0].id;
        e.set_caret(CursorPath {
            block,
            path: vec![Step::Index(1), Step::Body, Step::Index(0)],
            offset: 0,
        });
    };
    let mut e = Editor::default();
    e.insert_text("a #cl[xy] b");
    body_start(&mut e);
    assert!(e.backspace());
    assert!(e.call_marked());
    assert!(e.escape());
    assert!(!e.call_marked());
    assert_eq!(texts(&e), ["a #cl[xy] b"]);
    e.backspace();
    e.backspace();
    assert_eq!(texts(&e), ["a xy b"]);
    assert_eq!(
        (e.caret().path.clone(), e.caret().offset),
        (vec![Step::Index(0)], 2)
    );
    e.doc().check_invariants().unwrap();
    assert!(e.undo());
    assert_eq!(texts(&e), ["a #cl[xy] b"]);
    // A body with calls in it keeps them.
    let mut e = Editor::default();
    e.insert_text("a #audit[x *y* z] b");
    body_start(&mut e);
    e.backspace();
    e.backspace();
    assert_eq!(texts(&e), ["a x *y* z b"]);
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
    // In an empty call: it goes. At the start of a full one: marked, then
    // unwrapped (user request 2026-11-11; it used to be ←).
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
    assert!(e.call_marked());
    assert!(e.backspace());
    assert_eq!(texts(&e), ["a b"]);
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
    use Step::{Body, Index};
    assert_eq!(e.call_focus(), p(&[Index(1), Body, Index(1), Body]));
    e.insert_text("$");
    e.escape();
    assert_eq!(e.call_focus(), p(&[Index(1), Body]));
    e.insert_text(" `y");
    assert_eq!(e.call_focus(), p(&[Index(1), Body, Index(3)]));
    e.escape();
    e.escape();
    assert_eq!(e.call_focus(), None);
}

#[test]
fn inline_code() {
    let mut e = Editor::default();
    e.insert_text("run `a *b* $x` now");
    assert_eq!(texts(&e), ["run `a *b* $x` now"]);
    // Backspace at the start of the text after it selects it.
    caret_at(&mut e, 0, 2, 0);
    assert!(e.backspace());
    assert!(e.selected().is_some());
    // Edited in place: the caret goes into it with ←.
    e.escape();
    e.move_left();
    e.insert_text("!");
    assert_eq!(texts(&e), ["run `a *b* $x!` now"]);
    e.backspace();
    e.backspace();
    assert_eq!(texts(&e), ["run `a *b* $` now"]);
    // Empty code goes when left.
    let mut e = Editor::default();
    e.insert_text("a ``b");
    assert_eq!(texts(&e), ["a b"]);
    let mut e = Editor::default();
    e.insert_text("a `");
    e.move_left();
    assert_eq!(texts(&e), ["a "]);
    e.doc().check_invariants().unwrap();
    // `\`` is a literal backtick.
    let mut e = Editor::default();
    e.insert_text("a \\`");
    assert_eq!(texts(&e), ["a `"]);
}

#[test]
fn dashes_spaces_and_line_breaks() {
    let mut e = Editor::default();
    e.insert_text("1--2, a --- b, x~y, \\- \\~ - z");
    assert_eq!(texts(&e), ["1–2, a — b, x\u{a0}y, - ~ - z"]);
    // `\` then space, or Shift+Enter: a line break; spaces after it are
    // not kept (Typst drops them).
    let mut e = Editor::default();
    e.insert_text("a\\  b");
    assert!(e.line_break());
    e.insert_text(" c");
    assert_eq!(texts(&e), ["a\nb\nc"]);
    // Backspace takes it back like a char.
    caret_at(&mut e, 0, 0, 2);
    assert!(e.backspace());
    assert_eq!(texts(&e), ["ab\nc"]);
    // In a formula, Shift+Enter is a formula line break.
    let mut e = Editor::default();
    e.insert_text("$a");
    assert!(e.line_break());
    let Inline::Math(row) = &e.doc().blocks[0].kind.inlines().unwrap()[1].kind else {
        panic!()
    };
    assert_eq!(row.nodes[1].kind, MathKind::Linebreak);
}

/// Put the caret at the end of the label of the block it is in (or of the
/// environment around it), as a click on the pill does.
fn into_label(e: &mut Editor) {
    assert!(e.enter_label());
}

#[test]
fn block_labels() {
    let mut e = Editor::default();
    e.insert_text("= Intro");
    // → at the end of the heading does not enter its label; a click does.
    assert!(!e.move_right());
    into_label(&mut e);
    e.insert_text("sec:<in>tro");
    let label = |e: &Editor, b: usize| e.doc().blocks[b].label.clone();
    assert_eq!(label(&e, 0), "sec:intro", "only label chars");
    // ←/→ stay in it, Home/End go to its ends.
    assert!(!e.move_right());
    assert!(e.move_left());
    assert_eq!(e.tab_edge(true).map(|c| c.offset), Some(9));
    // Space leaves it, to the end of the heading.
    e.insert_text(" ");
    assert_eq!(e.caret(), &e.doc().end_of(&e.doc().blocks[0]));
    into_label(&mut e);
    e.backspace();
    e.backspace();
    e.backspace();
    e.backspace();
    e.backspace();
    e.backspace();
    assert_eq!(label(&e, 0), "sec");
    // Enter leaves it too; a display formula and an environment have one.
    e.enter();
    e.insert_text("$$x");
    into_label(&mut e);
    e.insert_text("eq");
    e.escape();
    let display = e
        .doc()
        .blocks
        .iter()
        .position(|b| matches!(b.kind, Block::DisplayMath(_)))
        .unwrap();
    assert_eq!(label(&e, display), "eq");
    // Paragraphs have none: → goes on.
    let mut e = Editor::default();
    e.insert_text("a");
    assert!(!e.move_right());
    // An environment's label: the environment stays outlined, and leaving
    // goes just after it.
    let mut e = Editor::default();
    e.insert_text("#lemma x");
    into_label(&mut e);
    assert_eq!(e.env_focus(), Some((e.caret().block, true)));
    e.insert_text("l1");
    assert!(e.escape());
    assert_eq!((e.caret().path.clone(), e.caret().offset), (vec![], 1));
    assert_eq!(texts(&e), ["#lemma[x]"]);
    assert_eq!(label(&e, 0), "l1");
    while e.undo() {}
    assert_eq!(label(&e, 0), "");
}

#[test]
fn references() {
    // Label chars go into the reference; a trailing `.` or `:` goes back to
    // the text, as Typst reads it.
    let mut e = Editor::default();
    e.insert_text("see @eq:abs. And @sec:a: done, @x");
    assert_eq!(texts(&e), ["see @{eq:abs}. And @{sec:a}: done, @{x}"]);
    assert_eq!(e.ref_typing(), Some("x".into()));
    // Backspace while typing takes chars, then the reference.
    e.backspace();
    assert_eq!(e.ref_typing(), Some("".into()));
    e.backspace();
    assert_eq!(texts(&e), ["see @{eq:abs}. And @{sec:a}: done, "]);
    // A finished reference is an atom: Backspace selects, then deletes it.
    let mut e = Editor::default();
    e.insert_text("a @b c");
    caret_at(&mut e, 0, 2, 0);
    assert!(e.backspace());
    assert!(e.selected().is_some());
    assert!(e.backspace());
    assert_eq!(texts(&e), ["a c"]);
    // `@` alone, or `\@`, is a literal `@`.
    let mut e = Editor::default();
    e.insert_text("a @ b \\@c");
    assert_eq!(texts(&e), ["a @ b @c"]);
    // Choosing from the list completes the label.
    let mut e = Editor::default();
    e.insert_text("see @e");
    assert!(e.complete_ref("eq:abs"));
    assert_eq!(e.ref_typing(), None);
    e.insert_text(" now");
    assert_eq!(texts(&e), ["see @{eq:abs} now"]);
    e.doc().check_invariants().unwrap();
    while e.undo() {}
    assert_eq!(texts(&e), [""]);
}

/// Following a reference goes to the labelled block (into an environment's
/// body); Alt+← comes back.
#[test]
fn following_references() {
    let mut e = Editor::default();
    e.insert_text("= Intro");
    into_label(&mut e);
    e.insert_text("sec");
    e.enter();
    e.enter();
    e.insert_text("#lemma x");
    into_label(&mut e);
    e.insert_text("lem");
    e.escape();
    e.insert_text("See @sec and @lem. ");
    let here = e.caret().clone();
    // The reference's id, as a click finds it.
    let r = e.doc().blocks[2].kind.inlines().unwrap()[3].id;
    assert_eq!(e.doc().ref_label(r), Some("lem"));
    assert!(e.follow_ref("lem"));
    assert_eq!(
        e.caret(),
        &e.doc().start_of(&e.doc().blocks[1].kind.body().unwrap()[0])
    );
    assert!(e.follow_ref("sec"));
    assert_eq!(e.caret().block, e.doc().blocks[0].id);
    assert!(e.go_back());
    assert!(e.go_back());
    assert_eq!(e.caret(), &here);
    assert!(!e.go_back());
    assert!(!e.follow_ref("nowhere"));
}

/// Hyperlinks are found at any depth, with the path into their text.
#[test]
fn links_in_a_block() {
    let mut e = Editor::default();
    e.insert_text("a #link(\"x\")[b] and *#link(\"y\")[c]* @r d");
    let block = &e.doc().blocks[0];
    use Step::{Body, Index};
    let paths: Vec<Vec<Step>> = block.links().into_iter().map(|(_, p)| p).collect();
    assert_eq!(
        paths,
        [vec![Index(1), Body], vec![Index(3), Body, Index(1), Body]]
    );
    assert_eq!(block.ref_ids().len(), 1);
}

/// `#figure ` starts a figure with an empty caption under its body; → goes
/// on into the caption, Enter in it leaves the figure; Backspace in an empty
/// caption removes it; unwrapping keeps the caption's text as a paragraph.
#[test]
fn bibliography_and_citations() {
    let mut e = Editor::default();
    e.insert_text("#lemma See @example. ");
    // Inserted from inside an environment: at the top level, after it.
    assert!(e.insert_block(NewBlock::Bibliography));
    assert_eq!(e.doc().blocks.len(), 2);
    assert!(matches!(e.doc().blocks[1].kind, Block::Bibliography(_)));
    assert_eq!(e.doc().bib_keys(), ["example"]);
    // Following a citation: to the bibliography; Alt+← back.
    let before = e.doc().start_of(&e.doc().blocks[0].kind.body().unwrap()[0]);
    e.set_caret(before.clone());
    assert!(e.follow_ref("example"));
    assert_eq!(e.caret().block, e.doc().blocks[1].id);
    assert!(e.go_back());
    assert_eq!(e.caret(), &before);
    assert!(!e.follow_ref("unknown"));
}

/// The title block goes to the start of the document, once; its fields
/// round-trip through its source.
#[test]
fn title_block() {
    let mut e = Editor::default();
    e.insert_text("Text");
    assert!(e.insert_block(NewBlock::Title));
    assert_eq!(texts(&e), ["title", "Text"]);
    let title = e.doc().blocks[0].id;
    assert_eq!(e.caret().block, title);
    let fields = e.doc().title().unwrap();
    assert_eq!((fields.title.as_str(), fields.authors.len()), ("Title", 1));
    // A second time: the caret goes to the one there is.
    let end = e.doc().end_of(&e.doc().blocks[1]);
    e.set_caret(end);
    assert!(e.insert_block(NewBlock::Title));
    assert_eq!(e.doc().blocks.len(), 2);
    assert_eq!(e.caret().block, title);

    let fields = TitleFields {
        title: "On $x$".into(),
        authors: vec!["A. B.".into(), "C".into()],
        date: "2026-11-10".into(),
    };
    assert_eq!(TitleFields::parse(&fields.source()), fields);
    assert_eq!(fields.ymd(), Some((2026, 11, 10)));
    let bad = TitleFields {
        date: "2026-13-01".into(),
        ..fields
    };
    assert_eq!(bad.ymd(), None);
    assert!(e.undo());
    assert_eq!(texts(&e), ["Text"]);
}

#[test]
fn figures_and_captions() {
    let mut e = Editor::default();
    e.insert_text("#figure Body");
    assert_eq!(texts(&e), ["#figure[Body|cap()]"]);
    assert!(e.move_right());
    e.insert_text("A caption");
    assert_eq!(texts(&e), ["#figure[Body|cap(A caption)]"]);
    assert!(e.hint().starts_with("caption"));
    // Enter: out of the figure, just after it.
    assert!(e.enter());
    assert_eq!(e.caret().path, Vec::<Step>::new());
    assert_eq!(e.caret().offset, 1);
    e.doc().check_invariants().unwrap();
    // Backspace at the start of a caption with text: ←, no join.
    let mut e = Editor::default();
    e.insert_text("#figure Body");
    e.move_right();
    e.insert_text("x");
    e.move_left();
    assert!(e.backspace());
    assert_eq!(texts(&e), ["#figure[Body|cap(x)]"]);
    // An empty caption goes.
    let mut e = Editor::default();
    e.insert_text("#figure Body");
    e.move_right();
    assert!(e.backspace());
    assert_eq!(texts(&e), ["#figure[Body]"]);
    // Unwrapping: the caption's text becomes a paragraph.
    let mut e = Editor::default();
    e.insert_text("#figure Body");
    e.move_right();
    e.insert_text("Cap");
    let body = e.doc().blocks[0].kind.body().unwrap()[0].clone();
    e.set_caret(e.doc().start_of(&body));
    assert!(e.backspace(), "marks the figure");
    assert!(e.backspace(), "unwraps it");
    assert_eq!(texts(&e), ["Body", "Cap"]);
    e.doc().check_invariants().unwrap();
    while e.undo() {}
    assert_eq!(texts(&e), [""]);
}

/// Shift + a move: the anchor stays, the caret moves.
fn select_with(e: &mut Editor, mut moves: impl FnMut(&mut Editor)) {
    let anchor = e.anchor();
    moves(e);
    e.set_anchor(anchor);
}

/// Range selections: Backspace deletes them, typing replaces them, Esc
/// cancels them; one undo step brings the text back.
#[test]
fn range_selections() {
    // Text: the last word.
    let mut e = Editor::default();
    e.insert_text("hello world");
    select_with(&mut e, |e| {
        for _ in 0..5 {
            e.move_left();
        }
    });
    assert!(matches!(
        e.selection(),
        Some(SelRange::Text {
            start: 6,
            end: 11,
            ..
        })
    ));
    assert!(e.backspace());
    assert_eq!(texts(&e), ["hello "]);
    assert!(e.undo());
    assert_eq!(texts(&e), ["hello world"]);
    // Typing over it; Esc cancels a range.
    let end = e.doc().doc_end();
    e.set_caret(end);
    select_with(&mut e, |e| _ = e.move_left());
    e.insert_text("D");
    assert_eq!(texts(&e), ["hello worlD"]);
    select_with(&mut e, |e| _ = e.move_left());
    assert!(e.escape());
    assert!(e.selection().is_none());
    // Across a formula: covered whole, the texts joined.
    let mut e = Editor::default();
    e.insert_text("ab $x$ cd");
    let block = e.doc().blocks[0].id;
    e.set_caret(CursorPath {
        block,
        path: vec![Step::Index(0)],
        offset: 1,
    });
    select_with(&mut e, |e| {
        e.set_caret(CursorPath {
            block,
            path: vec![Step::Index(2)],
            offset: 2,
        })
    });
    assert!(e.delete());
    assert_eq!(texts(&e), ["ad"]);
    e.doc().check_invariants().unwrap();
    assert!(e.undo());
    assert_eq!(texts(&e), ["ab $ cd"]);
    // Across blocks: whole blocks.
    let mut e = Editor::default();
    e.insert_text("one");
    e.enter();
    e.insert_text("two");
    e.enter();
    e.insert_text("three");
    select_with(&mut e, |e| {
        e.move_left();
        for _ in 0..7 {
            e.move_left();
        }
    });
    assert!(matches!(
        e.selection(),
        Some(SelRange::Blocks { start: 1, end: 3 })
    ));
    e.backspace();
    assert_eq!(texts(&e), ["one"]);
    // Math nodes: `/` typed over them wraps them.
    let mut e = empty_display();
    e.insert_text("a+b");
    select_with(&mut e, |e| {
        for _ in 0..3 {
            e.move_left();
        }
    });
    e.insert_text("/");
    assert!(
        display_of(&e, 0).starts_with("frac(a + b"),
        "{}",
        display_of(&e, 0)
    );
}

/// Paste: inlines join the text at both ends, blocks go between blocks (a
/// paragraph split at the caret), math nodes into the formula; a selected
/// range is replaced; one undo step.
#[test]
fn pasting() {
    let frag = |e: &mut Editor| {
        let mut d = Document::empty();
        let (x, y, z) = (d.text("X "), d.ident("y"), d.text(" Z"));
        let m = d.math(vec![y]);
        let _ = e;
        Fragment::Inlines(vec![x, m, z])
    };
    let mut e = Editor::default();
    e.insert_text("abcd");
    e.move_left();
    e.move_left();
    let f = frag(&mut e);
    assert!(e.paste(f));
    assert_eq!(texts(&e), ["abX $ Zcd"]);
    e.doc().check_invariants().unwrap();
    e.insert_text("!");
    assert_eq!(texts(&e), ["abX $ Z!cd"]);
    e.undo();
    assert!(e.undo());
    assert_eq!(texts(&e), ["abcd"]);
    // Over a selection.
    select_with(&mut e, |e| _ = e.move_left());
    let f = frag(&mut e);
    e.paste(f);
    assert_eq!(texts(&e), ["aX $ Zcd"]);
    assert!(e.undo());
    assert_eq!(texts(&e), ["abcd"]);
    // Blocks in the middle of a paragraph: it is split.
    let mut d = Document::empty();
    let t = d.text("one");
    let p1 = d.paragraph(vec![t]);
    let t = d.text("two");
    let p2 = d.paragraph(vec![t]);
    let mut e = Editor::default();
    e.insert_text("abcd");
    e.move_left();
    e.move_left();
    e.paste(Fragment::Blocks(vec![p1, p2]));
    assert_eq!(texts(&e), ["ab", "one", "two", "cd"]);
    e.insert_text("!");
    assert_eq!(texts(&e), ["ab", "one", "two!", "cd"]);
    e.undo();
    assert!(e.undo());
    assert_eq!(texts(&e), ["abcd"]);
    // Blocks into an empty paragraph replace it.
    let mut d = Document::empty();
    let t = d.text("one");
    let p1 = d.paragraph(vec![t]);
    let mut e = Editor::default();
    e.paste(Fragment::Blocks(vec![p1]));
    assert_eq!(texts(&e), ["one"]);
    // Math nodes into a formula.
    let mut e = empty_display();
    e.insert_text("a");
    let mut d = Document::empty();
    let (plus, b) = (d.sym("+"), d.ident("b"));
    e.paste(Fragment::Math(vec![plus, b]));
    assert_eq!(display_of(&e, 0), "a + b");
    e.insert_text("c");
    assert_eq!(display_of(&e, 0), "a + b c");
    e.doc().check_invariants().unwrap();
}

/// A remark's range follows edits: text typed before it moves it, text typed
/// at its end stays out, undo moves it back; deleting its text leaves a
/// remark on the whole block.
#[test]
fn remarks_follow_edits() {
    let mut e = Editor::default();
    e.insert_text("Hello world.");
    e.move_left();
    select_with(&mut e, |e| {
        for _ in 0..5 {
            e.move_left();
        }
    });
    let id = e.add_remark("Which world?", "A", "2026-11-13").unwrap();
    let block = e.doc().blocks[0].id;
    let range = |e: &Editor| {
        let r = e.doc().remarks.iter().find(|r| r.id == id).unwrap();
        let (a, b) = e.doc().remark_range(r).unwrap();
        (a.offset, b.offset)
    };
    assert_eq!(range(&e), (6, 11));
    e.set_caret(e.doc().start_of(&e.doc().blocks[0]));
    e.insert_text("Oh ");
    assert_eq!(range(&e), (9, 14));
    let at = |offset| CursorPath {
        block,
        path: vec![Step::Index(0)],
        offset,
    };
    e.set_caret(at(14));
    e.insert_text("s");
    assert_eq!(range(&e), (9, 14), "typed at the end: not in it");
    e.set_caret(at(9));
    e.insert_text("big ");
    assert_eq!(range(&e), (13, 18), "typed at the start: not in it");
    assert!(e.undo() && e.undo() && e.undo());
    assert_eq!(texts(&e), ["Hello world."]);
    assert_eq!(range(&e), (6, 11));
    assert!(e.redo());
    assert_eq!(range(&e), (9, 14));
    // Deleting the remarked text: the remark is on the block.
    e.set_caret(at(14));
    select_with(&mut e, |e| {
        for _ in 0..5 {
            e.move_left();
        }
    });
    assert!(e.backspace());
    assert_eq!(texts(&e), ["Oh Hello ."]);
    assert_eq!(e.doc().remarks[0].anchor, Anchor::Block(block));
    e.doc().check_invariants().unwrap();
}

/// Without a selection a remark is on the top-level block the caret is in:
/// a whole theorem, not its paragraph.
#[test]
fn remarks_on_blocks() {
    let mut d = Document::empty();
    let t = d.text("Text.");
    let p = d.paragraph(vec![t]);
    d.blocks = vec![d.env("lemma", vec![p])];
    let mut e = Editor::new(d);
    let env = e.doc().blocks[0].id;
    e.set_caret(e.doc().start_of(&e.doc().blocks[0].kind.body().unwrap()[0]));
    e.add_remark("Check.", "A", "2026-11-13").unwrap();
    assert_eq!(e.doc().remarks[0].anchor, Anchor::Block(env));
    assert_eq!(e.doc().remark_range(&e.doc().remarks[0]), None);
}

/// A range in a formula follows edits of its row; a structure made around
/// part of the row keeps it in place.
#[test]
fn remarks_in_formulas() {
    let mut e = Editor::default();
    e.insert_text("$$a + b + c");
    // Select `b`: nodes 2..3 of the row.
    let block = e.doc().blocks[0].id;
    let at = |offset| CursorPath {
        block,
        path: vec![],
        offset,
    };
    e.set_caret(at(2));
    select_with(&mut e, |e| {
        e.set_caret(at(3));
    });
    let id = e.add_remark("b?", "A", "2026-11-13").unwrap();
    let range = |e: &Editor| {
        let r = e.doc().remarks.iter().find(|r| r.id == id).unwrap();
        let (a, b) = e.doc().remark_range(r).unwrap();
        (a.offset, b.offset)
    };
    assert_eq!(range(&e), (2, 3));
    e.set_caret(at(0));
    e.insert_text("x ");
    assert_eq!(range(&e), (3, 4));
    // `/` after `b` makes `b` a numerator: the range holds the fraction.
    e.set_caret(at(4));
    e.insert_text("/");
    assert!(matches!(
        e.doc().blocks[0].kind,
        Block::DisplayMath(ref r) if matches!(r.nodes[3].kind, MathKind::Frac { .. })
    ));
    assert_eq!(range(&e), (3, 4));
    while e.undo() {}
    e.doc().check_invariants().unwrap();
}

/// The sidebar's view of remarks: in text order, the ones the caret is in,
/// selecting one, replies, resolving and deleting.
#[test]
fn remarks_for_the_sidebar() {
    let mut e = Editor::default();
    e.insert_text("First.");
    e.enter();
    e.insert_text("Second block.");
    let (b0, b1) = (e.doc().blocks[0].id, e.doc().blocks[1].id);
    let at = |block, offset| CursorPath {
        block,
        path: vec![Step::Index(0)],
        offset,
    };
    // A range in the second block ("block"), then a remark on the first.
    e.set_caret(at(b1, 7));
    select_with(&mut e, |e| e.set_caret(at(b1, 12)));
    let range = e.add_remark("Which?", "A", "2026-11-14").unwrap();
    e.set_caret(at(b0, 2));
    let block = e.add_remark("Whole.", "A", "2026-11-14").unwrap();
    let order: Vec<u64> = e.doc().remarks_in_order().iter().map(|r| r.id).collect();
    assert_eq!(order, [block, range]);

    assert_eq!(e.remarks_at_caret(), [block]);
    e.set_caret(at(b1, 3));
    assert_eq!(e.remarks_at_caret(), [] as [u64; 0]);
    e.set_caret(at(b1, 12));
    assert_eq!(e.remarks_at_caret(), [range], "the end is in it");

    assert!(e.select_remark(range));
    let (a, b) = e.selection_bounds().unwrap();
    assert_eq!((a.offset, b.offset), (7, 12));
    assert!(e.select_remark(block));
    assert_eq!(e.selection(), None);
    assert_eq!(e.caret().block, b0);

    assert!(e.reply_remark(range, "This one.", "B", "2026-11-14"));
    assert_eq!(e.doc().remarks[0].replies[0].author, "B");
    assert!(e.set_remark_resolved(range, true));
    e.set_caret(at(b1, 9));
    assert_eq!(e.remarks_at_caret(), [] as [u64; 0], "resolved: not in it");
    assert!(e.delete_remark(range));
    assert!(!e.delete_remark(range));
    assert_eq!(e.doc().remarks.len(), 1);
}
