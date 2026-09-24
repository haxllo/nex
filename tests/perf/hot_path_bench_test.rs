// Query hot-path benchmark.
//
// Reports p50/p95 latency for the query shapes the ranking pass actually
// sees, so ranking changes can be judged against numbers instead of vibes.
// Run with:
//
//   cargo test -p nex --test perf_hot_path_bench_test -- --nocapture

use std::time::Instant;

use nex_core::model::SearchItem;
use nex_core::search::search;

fn percentile(samples: &mut [f64], pct: f64) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let last = samples.len().saturating_sub(1);
    let idx = ((last as f64) * pct).round() as usize;
    samples[idx.min(last)]
}

fn corpus(size: usize) -> Vec<SearchItem> {
    let mut items: Vec<SearchItem> = (0..size)
        .map(|i| {
            SearchItem::new(
                &format!("file-{i}"),
                "file",
                &format!("Document_{i:05}.txt"),
                &format!("C:\\Docs\\Document_{i:05}.txt"),
            )
        })
        .collect();

    // A handful of realistic apps so the app-intent bonus path is exercised.
    for (id, title, path) in [
        ("app-code", "Visual Studio Code", "C:\\Program Files\\Microsoft VS Code\\Code.exe"),
        ("app-term", "Windows Terminal", "C:\\Program Files\\WindowsApps\\Terminal.exe"),
        ("app-ff", "Firefox", "C:\\Program Files\\Mozilla Firefox\\firefox.exe"),
        ("app-calc", "Calculator", "C:\\Windows\\System32\\calc.exe"),
    ] {
        items.push(SearchItem::new(id, "app", title, path));
    }

    items.push(SearchItem::new(
        "q4",
        "file",
        "Q4_Report.xlsx",
        "C:\\Reports\\Q4_Report.xlsx",
    ));
    items
}

fn measure<F: FnMut()>(iterations: usize, mut body: F) -> (f64, f64) {
    for _ in 0..10 {
        body();
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        body();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    (percentile(&mut samples, 0.50), percentile(&mut samples, 0.95))
}

#[test]
fn report_hot_path_latency_by_query_shape() {
    const ITERATIONS: usize = 40;
    let items = corpus(20_000);

    let shapes = [
        ("single-char prefix", "d"),
        ("one-token prefix", "doc"),
        ("two-token fuzzy (gate query)", "q4 reort"),
        ("long substring", "report"),
        ("no-match (worst scan)", "zzzzzzzz"),
    ];

    println!("\n=== query hot-path latency (20k items, debug build) ===");
    let mut worst_full_scan = 0.0_f64;
    for (label, query) in shapes {
        let (p50, p95) = measure(ITERATIONS, || {
            let _ = search(&items, query, 20);
        });
        // The reject-heavy shapes are the ones the presence-mask optimisation
        // targets; they should stay far below the full-match shapes.
        if matches!(label, "two-token fuzzy (gate query)" | "no-match (worst scan)") {
            worst_full_scan = worst_full_scan.max(p95);
        }
        println!("  {label:<32} p50={p50:>8.3}ms  p95={p95:>8.3}ms");
    }

    // Reference point for the predictive pre-fetch cache: once a
    // single-character result set is pre-computed, serving it is a clone of
    // the cached rows rather than a ranking pass over the whole corpus.
    let cached = search(&items, "d", 64);
    assert!(!cached.is_empty());
    let (cache_p50, cache_p95) = measure(ITERATIONS, || {
        let served: Vec<SearchItem> = cached.clone();
        std::hint::black_box(served.len());
    });
    println!(
        "  {:<32} p50={cache_p50:>8.3}ms  p95={cache_p95:>8.3}ms  (pre-fetch cache hit, 64 rows)",
        "single-char served from cache"
    );
    println!("=====================================================\n");

    // The presence-mask reject is the concrete guarantee: queries whose
    // characters are largely absent from the corpus must not pay a full
    // lexical scan. 10x headroom keeps this stable on slow CI.
    assert!(
        worst_full_scan <= 25.0,
        "presence-mask reject regressed: worst p95 {worst_full_scan:.3}ms (ceiling 25ms)"
    );
}
