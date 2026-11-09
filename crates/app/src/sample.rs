//! The sample document: a short text on matrix product states that uses
//! every feature of the editor. The first sections are built node by node;
//! the last one is typed through the editor, as a user would.

use model::{Document, Editor, InlineNode, ListKind, MathNode, NewBlock};

/// Short helpers so the sample reads roughly like the formulas it builds.
struct B(Document);

impl B {
    fn t(&mut self, s: &str) -> InlineNode {
        self.0.text(s)
    }
    fn i(&mut self, s: &str) -> MathNode {
        self.0.ident(s)
    }
    fn n(&mut self, s: &str) -> MathNode {
        self.0.num(s)
    }
    fn s(&mut self, s: &str) -> MathNode {
        self.0.sym(s)
    }
    fn frac(&mut self, num: Vec<MathNode>, den: Vec<MathNode>) -> MathNode {
        self.0.frac(num, den)
    }
    fn sup(&mut self, base: MathNode, sup: Vec<MathNode>) -> MathNode {
        self.0.scripts(vec![base], Some(sup), None)
    }
    fn sub(&mut self, base: MathNode, sub: Vec<MathNode>) -> MathNode {
        self.0.scripts(vec![base], None, Some(sub))
    }
    fn m(&mut self, nodes: Vec<MathNode>) -> InlineNode {
        self.0.math(nodes)
    }
}

pub fn document() -> Document {
    let mut b = B(Document::empty());
    let mut blocks = vec![];

    // ----- 1 Matrix product states -----
    let t = b.t("Matrix product states");
    let mut heading = b.0.heading(1, vec![t]);
    heading.label = "sec:mps".into();
    blocks.push(heading);

    let t1 = b.t("Consider a chain of ");
    let n = b.i("n");
    let m1 = b.m(vec![n]);
    let t2 = b.t(" sites with local dimension ");
    let d = b.i("d");
    let m2 = b.m(vec![d]);
    let t3a = b.t(". A ");
    let term = b.t("matrix product state");
    let term = b.0.inline_call("emph", "", vec![term]);
    let t3 = b.t(" is given by matrices ");
    let a = b.i("A");
    let i = b.i("i");
    let ai = b.sup(a, vec![i]);
    let m3 = b.m(vec![ai]);
    let t4 = b.t(" of size ");
    let (chi, times, chi2) = (b.s("χ"), b.s("×"), b.s("χ"));
    let m4 = b.m(vec![chi, times, chi2]);
    let t5 = b.t(", one for every basis state of a site; its coefficients are");
    blocks.push(b.0.paragraph(vec![t1, m1, t2, m2, t3a, term, t3, m3, t4, m4, t5]));

    let site = |b: &mut B, k: &str| {
        let i = b.i("i");
        let k = if k.chars().all(|c| c.is_ascii_digit()) {
            b.n(k)
        } else {
            b.i(k)
        };
        b.sub(i, vec![k])
    };
    let psi = b.s("ψ");
    let (i1, dots, i_n) = (site(&mut b, "1"), b.s("…"), site(&mut b, "n"));
    let psi_i = b.sub(psi, vec![i1, dots, i_n]);
    let eq = b.s("=");
    let tr = b.i("tr");
    let lp = b.s("(");
    let a1 = {
        let a = b.i("A");
        let i1 = site(&mut b, "1");
        b.sup(a, vec![i1])
    };
    let a2 = {
        let a = b.i("A");
        let i2 = site(&mut b, "2");
        b.sup(a, vec![i2])
    };
    let cdots = b.s("⋯");
    let an = {
        let a = b.i("A");
        let i_n = site(&mut b, "n");
        b.sup(a, vec![i_n])
    };
    let rp = b.s(")");
    let mut mps = b.0.display(vec![psi_i, eq, tr, lp, a1, a2, cdots, an, rp]);
    mps.label = "eq:mps".into();
    blocks.push(mps);

    let t1 = b.t("The bond dimension ");
    let chi = b.s("χ");
    let m1 = b.m(vec![chi]);
    let t2 = b.t(
        " controls how much entanglement the state can carry. Graphically, every matrix is a \
         tensor with one physical leg and two virtual legs, and contracted virtual legs form the chain:",
    );
    blocks.push(b.0.paragraph(vec![t1, m1, t2]));
    blocks.push(b.0.diagram(
        "for i in range(4) {
  let x = i * 1.6
  line((x, -0.35), (x, -0.9))
  circle((x, 0), radius: 0.35, fill: rgb(\"#dbe8fb\"), name: \"A\" + str(i))
  content((x, 0), $A$)
}
for i in range(3) {
  line(\"A\" + str(i) + \".east\", \"A\" + str(i + 1) + \".west\")
}",
    ));
    let t = b.t("Two cases to keep in mind:");
    blocks.push(b.0.paragraph(vec![t]));
    let t1 = b.t("Product states have ");
    let (chi, eq, one) = (b.s("χ"), b.s("="), b.n("1"));
    let m = b.m(vec![chi, eq, one]);
    let t2 = b.t(" and carry no entanglement.");
    let p = b.0.paragraph(vec![t1, m, t2]);
    blocks.push(b.0.item(ListKind::Bullet, vec![p]));
    let t = b.t("Small entangled examples:");
    let p = b.0.paragraph(vec![t]);
    let mut examples = vec![p];
    for s in [
        "the GHZ state, with bond dimension 2;",
        "the AKLT state, the ground state of a spin-1 chain.",
    ] {
        let t = b.t(s);
        let p = b.0.paragraph(vec![t]);
        examples.push(b.0.item(ListKind::Numbered, vec![p]));
    }
    blocks.push(b.0.item(ListKind::Bullet, examples));

    // ----- 2 Transfer matrix -----
    let t = b.t("The transfer matrix");
    blocks.push(b.0.heading(1, vec![t]));

    let t1 = b.t("Local expectation values and correlations are governed by the transfer matrix");
    blocks.push(b.0.paragraph(vec![t1]));
    let e = b.i("E");
    let eq = b.s("=");
    let sum = {
        let sum = b.s("∑");
        let i = b.i("i");
        b.sub(sum, vec![i])
    };
    let ai = {
        let a = b.i("A");
        let i = b.i("i");
        b.sup(a, vec![i])
    };
    let otimes = b.s("⊗");
    let conj = {
        let (lp, rp) = (b.s("("), b.s(")"));
        let a = b.i("A");
        let i = b.i("i");
        let ai = b.sup(a, vec![i]);
        let star = b.s("∗");
        b.0.scripts(vec![lp, ai, rp], Some(vec![star]), None)
    };
    blocks.push(b.0.display(vec![e, eq, sum, ai, otimes, conj]));

    let t1 = b.t("drawn as a tensor and its complex conjugate joined along the physical leg:");
    blocks.push(b.0.paragraph(vec![t1]));
    blocks.push(b.0.diagram(
        "for (y, fill) in ((0.6, \"#dbe8fb\"), (-0.6, \"#fde2e2\")) {
  line((-1.1, y), (1.1, y))
  circle((0, y), radius: 0.35, fill: rgb(fill))
}
content((0, 0.6), $A$)
content((0, -0.6), $overline(A)$)
line((0, 0.25), (0, -0.25))",
    ));

    let t1 = b.t("If ");
    let l1 = {
        let l = b.s("λ");
        let one = b.n("1");
        b.sub(l, vec![one])
    };
    let m1 = b.m(vec![l1]);
    let t2 = b.t(" and ");
    let l2 = {
        let l = b.s("λ");
        let two = b.n("2");
        b.sub(l, vec![two])
    };
    let m2 = b.m(vec![l2]);
    let t3 = b.t(
        " are the two largest eigenvalues of the transfer matrix, connected correlations at \
         distance ",
    );
    let r = b.i("r");
    let m3 = b.m(vec![r]);
    let t4 = b.t(" decay exponentially,");
    blocks.push(b.0.paragraph(vec![t1, m1, t2, m2, t3, m3, t4]));
    let c = b.i("C");
    let (lp, r, rp) = (b.s("("), b.i("r"), b.s(")"));
    let sim = b.s("∼");
    let ratio = {
        let (lp, rp) = (b.s("("), b.s(")"));
        let l2 = {
            let l = b.s("λ");
            let two = b.n("2");
            b.sub(l, vec![two])
        };
        let l1 = {
            let l = b.s("λ");
            let one = b.n("1");
            b.sub(l, vec![one])
        };
        let f = b.frac(vec![l2], vec![l1]);
        let r = b.i("r");
        b.0.scripts(vec![lp, f, rp], Some(vec![r]), None)
    };
    blocks.push(b.0.display(vec![c, lp, r, rp, sim, ratio]));

    let t1 = b.t("so the correlation length is ");
    let xi = b.s("ξ");
    let eq = b.s("=");
    let f = {
        let one = b.n("1");
        let log = b.i("log");
        let l1 = {
            let l = b.s("λ");
            let one = b.n("1");
            b.sub(l, vec![one])
        };
        let slash = b.s("/");
        let l2 = {
            let l = b.s("λ");
            let two = b.n("2");
            b.sub(l, vec![two])
        };
        b.frac(vec![one], vec![log, l1, slash, l2])
    };
    let m1 = b.m(vec![xi, eq, f]);
    let t2 = b.t(".");
    blocks.push(b.0.paragraph(vec![t1, m1, t2]));

    // ----- 3 Entanglement -----
    let t = b.t("Entanglement");
    blocks.push(b.0.heading(1, vec![t]));
    let t = b.t("Area law");
    blocks.push(b.0.heading(2, vec![t]));

    let t1 = b.t(
        "Cutting the chain at one bond, the entanglement entropy of a state with bond dimension ",
    );
    let chi = b.s("χ");
    let m1 = b.m(vec![chi]);
    let t2 = b.t(" is bounded independently of the system size:");
    blocks.push(b.0.paragraph(vec![t1, m1, t2]));
    let (s, le, log, chi) = (b.i("S"), b.s("≤"), b.i("log"), b.s("χ"));
    blocks.push(b.0.display(vec![s, le, log, chi]));

    let t1 = b.t("In terms of the Schmidt values ");
    let lam = b.s("λ");
    let i = b.i("i");
    let li = b.sub(lam, vec![i]);
    let m1 = b.m(vec![li]);
    let t2 = b.t(", normalized as ");
    let lam = b.s("λ");
    let i = b.i("i");
    let two = b.n("2");
    let li2 = b.0.scripts(vec![lam], Some(vec![two]), Some(vec![i]));
    let sum = b.s("∑");
    let eq = b.s("=");
    let one = b.n("1");
    let m2 = b.m(vec![sum, li2, eq, one]);
    let t3 = b.t(", the entropy is");
    blocks.push(b.0.paragraph(vec![t1, m1, t2, m2, t3]));

    let (s, eq, minus, sum) = (b.i("S"), b.s("="), b.s("−"), b.s("∑"));
    let lam = b.s("λ");
    let i = b.i("i");
    let two = b.n("2");
    let li2 = b.0.scripts(vec![lam], Some(vec![two]), Some(vec![i]));
    let log = b.i("log");
    let lam = b.s("λ");
    let i = b.i("i");
    let two = b.n("2");
    let li2b = b.0.scripts(vec![lam], Some(vec![two]), Some(vec![i]));
    blocks.push(b.0.display(vec![s, eq, minus, sum, li2, log, li2b]));

    let t = b.t("Uniqueness of the tensors");
    blocks.push(b.0.heading(2, vec![t]));
    let t1 = b.t(
        "Different tensors can generate the same state. The statement below is an \
         environment: its text is edited in place, while the code block before it (the \
         marker; double-click to edit) defines how theorems and proofs look.",
    );
    blocks.push(b.0.paragraph(vec![t1]));
    blocks.push(b.0.raw(
        "// Theorems: figures of their own kind, numbered on their own, with an
// optional title: #theorem[Title][Statement] (as in ctheorems).
#let theorem(..args) = {
  let (title, body) = if args.pos().len() > 1 { args.pos() } else { (none, ..args.pos()) }
  figure(kind: \"theorem\", supplement: \"Theorem\", numbering: \"1\", outlined: false,
    caption: title, body)
}
#show figure.where(kind: \"theorem\"): it => block(width: 100%, above: 1em, below: 1em,
  align(left)[*Theorem #it.counter.display(it.numbering)#if it.caption != none [ (#it.caption.body)].* #emph(it.body)])
#let proof(..args) = {
  let (title, body) = if args.pos().len() > 1 { args.pos() } else { (none, ..args.pos()) }
  block(width: 100%)[_Proof#if title != none [ (#title)]._ #body #h(1fr) $square$]
}",
    ));

    let t1 = b.t("If two MPS with injective tensors ");
    let a = b.i("A");
    let m1 = b.m(vec![a]);
    let t2 = b.t(" and ");
    let bb = b.i("B");
    let m2 = b.m(vec![bb]);
    let t3 = b.t(" generate the same state for every system size, then ");
    let bi = {
        let bb = b.i("B");
        let i = b.i("i");
        b.sup(bb, vec![i])
    };
    let eq = b.s("=");
    let phase = {
        let e = b.i("e");
        let (i, phi) = (b.i("i"), b.s("φ"));
        b.sup(e, vec![i, phi])
    };
    let x = b.i("X");
    let ai = {
        let a = b.i("A");
        let i = b.i("i");
        b.sup(a, vec![i])
    };
    let xinv = {
        let x = b.i("X");
        let (minus, one) = (b.s("−"), b.n("1"));
        b.sup(x, vec![minus, one])
    };
    let m3 = b.m(vec![bi, eq, phase, x, ai, xinv]);
    let t4 = b.t(" for an invertible matrix ");
    let x = b.i("X");
    let m4 = b.m(vec![x]);
    let t5 = b.t(" and a phase ");
    let phi = b.s("φ");
    let m5 = b.m(vec![phi]);
    let t6 = b.t(".");
    let statement =
        b.0.paragraph(vec![t1, m1, t2, m2, t3, m3, t4, m4, t5, m5, t6]);
    let mut theorem = b.0.env("theorem", vec![statement]);
    let title = b.t("Fundamental theorem of MPS");
    if let model::Block::Env { title: t, .. } = &mut theorem.kind {
        t.push(title);
    }
    theorem.label = "thm:fundamental".into();
    blocks.push(theorem);

    let t1 = b.t(
        "Equal states have equal transfer matrices, which fixes the tensors up to a gauge \
         transformation; injectivity makes the gauge invertible and unique up to the phase.",
    );
    let p = b.0.paragraph(vec![t1]);
    blocks.push(b.0.env("proof", vec![p]));

    b.0.blocks = blocks;
    b.0.header = format!(
        "{}#set math.equation(numbering: \"(1)\")\n#set par(justify: true)\n\
         // A macro for formulas: ket(psi) is |ψ⟩.\n\
         #let ket(x) = $lr(|#x chevron.r)$\n",
        typst_bridge::DEFAULT_HEADER
    );
    typed_section(b.0)
}

/// The last section, typed key by key at the end of `doc`.
fn typed_section(doc: Document) -> Document {
    let mut e = Editor::new(doc);
    e.set_symbol_resolver(typst_bridge::resolve_symbol);
    e.set_call_resolver(typst_bridge::call_shape);
    e.set_math_call_resolver(typst_bridge::math_call_shape);
    let end = e.doc().doc_end();
    e.set_caret(end);

    // A heading with a label: → at its end goes onto the label's pill.
    e.insert_text("= Notation and markup");
    e.move_right();
    e.insert_text("sec:notation");
    e.enter();

    // Markup in text.
    e.insert_text("Text can be *bold*, _italic_, #underline[underlined] or a ");
    e.insert_text("#link(\"https://typst.app\")[link]; code is `cargo test`. ");
    e.insert_text("Dashes join ranges, pages 3--7, or set off a pause --- like this. ");
    e.insert_text("A non-breaking space keeps Fig.~1 together.\\ ");
    e.insert_text("A line break starts this line.");
    e.enter();

    // Calls and atoms in formulas, and an inline diagram.
    e.insert_text("Formulas call functions: $cal(H) = overline(z) + hat(x)$, ");
    e.insert_text("$sqrt(2) + root(3, x)$, with primes $f'(x) + g''$, ");
    e.insert_text("text $x \"if\" y$ and the header's macros, $ket(psi)$. ");
    e.insert_text("A diagram can sit in a formula: $D = ");
    e.insert_block(NewBlock::Diagram);
    e.insert_text("$.");
    e.enter();

    // Labelled display formulas: `$$`, the formula, → onto the label.
    // Enter in `cases` starts the next case.
    e.insert_text("$$|x| = cases(x \"if\" x >= 0");
    e.enter();
    e.insert_text("-x \"otherwise\")");
    e.move_right();
    e.insert_text("eq:abs");
    e.enter();
    e.insert_text("Aligned equations put `&` where lines align and `\\` between them:");
    e.enter();
    // Esc leaves a superscript.
    e.insert_text("$$(b + c)^2");
    e.escape();
    e.insert_text(" &= (b + c)(b + c) \\ &= b^2");
    e.escape();
    e.insert_text(" + 2 b c + c^2");
    e.escape();
    e.move_right();
    e.insert_text("eq:square");
    e.enter();

    // A term list: `/ `, the term, `:` on to its description. Enter starts
    // the next item (caret in its term); Enter in an empty term goes on to
    // the description, and Enter on an empty description leaves the list.
    e.insert_text("/ Bond dimension: the size of the virtual legs.");
    e.enter();
    e.insert_text("Gauge: an invertible matrix on a virtual leg.");
    e.enter();
    e.enter();
    e.enter();

    // A block call with an argument; Esc leaves it.
    e.insert_text("#align(center) _A block call with an argument._");
    e.escape();
    e.insert_text("Headings, formulas and theorems carry labels, shown beside the page.");
    e.doc().clone()
}
