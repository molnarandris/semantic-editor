//! Times cold and warm compile + render of sample formula blocks.

use std::time::{Duration, Instant};

use typst_bridge::{Around, BridgeWorld, block_document, compile, render_page};

const WIDTH_PT: f64 = 400.0;
const PIXEL_PER_PT: f64 = 2.0;
const WARM_RUNS: usize = 20;

const FORMULAS: [(&str, &str); 5] = [
    ("entropy bound", "$ S <= (log chi)/(2) + c $"),
    (
        "quadratic",
        "$ x = (-b plus.minus sqrt(b^2 - 4 a c))/(2 a) $",
    ),
    ("sum", "$ sum_(i=1)^n i^2 = (n (n+1) (2n+1))/(6) $"),
    (
        "integral",
        "$ integral_0^infinity e^(-x^2) dif x = (sqrt(pi))/(2) $",
    ),
    (
        "matrix",
        "$ rho = mat(a, b; c, d) times.o mat(1, 0; 0, 1) $",
    ),
];

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn main() {
    let t = Instant::now();
    let mut world = BridgeWorld::new();
    println!(
        "world creation (fonts + library): {:8.2} ms\n",
        ms(t.elapsed())
    );

    println!(
        "{:<14} {:>12} {:>12} {:>12} {:>12}",
        "formula", "cold c+r", "warm comp", "warm render", "warm total"
    );
    for (name, snippet) in FORMULAS {
        let (doc_text, _) = block_document(&Around::default(), snippet, WIDTH_PT);

        let t = Instant::now();
        let doc = compile(&mut world, &doc_text).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let raster = render_page(&doc.pages()[0], PIXEL_PER_PT);
        let cold = t.elapsed();
        assert!(raster.width > 0 && raster.height > 0);

        let (mut comps, mut rends, mut totals) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..WARM_RUNS {
            // Recompile from a fresh string, as the editor will after an edit.
            let t = Instant::now();
            let doc = compile(&mut world, &doc_text).unwrap();
            let c = t.elapsed();
            let t = Instant::now();
            let _ = render_page(&doc.pages()[0], PIXEL_PER_PT);
            let r = t.elapsed();
            comps.push(c);
            rends.push(r);
            totals.push(c + r);
        }
        println!(
            "{:<14} {:>9.2} ms {:>9.2} ms {:>9.2} ms {:>9.2} ms   ({}x{} px)",
            name,
            ms(cold),
            ms(median(comps)),
            ms(median(rends)),
            ms(median(totals)),
            raster.width,
            raster.height,
        );
    }

    // A "keystroke" scenario: a formula that changes slightly every time.
    let mut times = Vec::new();
    for i in 0..WARM_RUNS {
        let (doc_text, _) = block_document(
            &Around::default(),
            &format!("$ S <= (log chi)/(2) + c_{i} $"),
            WIDTH_PT,
        );
        let t = Instant::now();
        let doc = compile(&mut world, &doc_text).unwrap();
        let _ = render_page(&doc.pages()[0], PIXEL_PER_PT);
        times.push(t.elapsed());
    }
    println!(
        "\nedited formula (median of {WARM_RUNS}): {:.2} ms",
        ms(median(times))
    );
}
