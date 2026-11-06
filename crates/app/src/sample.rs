//! A hard-coded document for the viewer milestone.

use model::{Document, InlineNode, MathNode};

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

    let t = b.t("Entanglement in matrix product states");
    blocks.push(b.0.heading(1, vec![t]));

    let t1 = b.t("Let ");
    let psi = b.s("ψ");
    let m1 = b.m(vec![psi]);
    let t2 = b.t(" be a matrix product state with bond dimension ");
    let chi = b.s("χ");
    let m2 = b.m(vec![chi]);
    let t3 = b.t(". Cutting the chain at one bond, the entanglement entropy satisfies");
    blocks.push(b.0.paragraph(vec![t1, m1, t2, m2, t3]));

    let (s, le, log, chi, two, plus, c) = (
        b.i("S"),
        b.s("≤"),
        b.i("log"),
        b.s("χ"),
        b.n("2"),
        b.s("+"),
        b.i("c"),
    );
    let f = b.frac(vec![log, chi], vec![two]);
    blocks.push(b.0.display(vec![s, le, f, plus, c]));

    let t1 = b.t("where ");
    let c = b.i("c");
    let m = b.m(vec![c]);
    let t2 = b.t(" is a constant independent of the system size. The Schmidt values ");
    let lam = b.s("λ");
    let i = b.i("i");
    let li = b.sub(lam, vec![i]);
    let m2 = b.m(vec![li]);
    let t3 = b.t(" are normalized, ");
    let lam = b.s("λ");
    let i = b.i("i");
    let two = b.n("2");
    let li2 = b.0.scripts(vec![lam], Some(vec![two]), Some(vec![i]));
    let sum = b.s("∑");
    let eq = b.s("=");
    let one = b.n("1");
    let m3 = b.m(vec![sum, li2, eq, one]);
    let t4 = b.t(", and the entropy is");
    blocks.push(b.0.paragraph(vec![t1, m, t2, m2, t3, m3, t4]));

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

    let t = b.t("Empty slots render as dashed boxes, so incomplete formulas keep their shape:");
    blocks.push(b.0.paragraph(vec![t]));
    let x = b.i("x");
    let empty = b.frac(vec![], vec![]);
    let xs = b.sup(x, vec![empty]);
    let eq = b.s("=");
    let half = b.frac(vec![], vec![]);
    blocks.push(b.0.display(vec![xs, eq, half]));
    blocks.push(b.0.raw("#align(center, text(fill: luma(110), style: \"italic\")[A raw Typst block, emitted verbatim.])"));

    let t = b.t("A diagram block holds CeTZ drawing code; here the state as a chain of tensors:");
    blocks.push(b.0.paragraph(vec![t]));
    blocks.push(b.0.diagram(
        "for i in range(4) {
  let x = i * 1.6
  line((x, -0.35), (x, -0.9))
  circle((x, 0), radius: 0.35, fill: rgb(\"#dbe8fb\"), name: \"A\" + str(i))
  content((x, 0), $A_#(i + 1)$)
}
for i in range(3) {
  line(\"A\" + str(i) + \".east\", \"A\" + str(i + 1) + \".west\")
}",
    ));

    for k in 1..=12 {
        let t = b.t(&format!("Scaling check {k}"));
        blocks.push(b.0.heading(2, vec![t]));
        let t1 = b.t("For a gapped chain the correlation length ");
        let xi = b.s("ξ");
        let m1 = b.m(vec![xi]);
        let t2 = b.t(&format!(
            " stays finite, and the bond dimension needed for accuracy ε grows only polynomially. \
             Paragraph {k} is filler text so the window has something to scroll through."
        ));
        blocks.push(b.0.paragraph(vec![t1, m1, t2]));
        let (chi, sim) = (b.s("χ"), b.s("∼"));
        let one = b.n("1");
        let kk = b.n(&k.to_string());
        let minus = b.s("−");
        let f = b.frac(vec![one], vec![kk]);
        let eps2 = b.s("ε");
        let pow = b.sup(eps2, vec![minus, f]);
        blocks.push(b.0.display(vec![chi, sim, pow]));
    }

    b.0.blocks = blocks;
    b.0
}
