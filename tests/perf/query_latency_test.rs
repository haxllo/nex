use std::time::Instant;

use nex_core::model::SearchItem;
use nex_core::search::search;

fn p95_ms(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let last = samples.len().saturating_sub(1);
    let idx = ((last as f64) * 0.95).round() as usize;
    samples[idx.min(last)]
}

#[test]
fn warm_query_p95_under_15ms() {
    let mut items: Vec<SearchItem> = (0..10_000)
        .map(|i| {
            SearchItem::new(
                &i.to_string(),
                "file",
                &format!("Document_{i:05}.txt"),
                &format!("C:\\Docs\\Document_{i:05}.txt"),
            )
        })
        .collect();

    items.push(SearchItem::new(
        "q4",
        "file",
        "Q4_Report.xlsx",
        "C:\\Reports\\Q4_Report.xlsx",
    ));

    for _ in 0..30 {
        let _ = search(&items, "q4 reort", 20);
    }

    let mut batch_p95 = Vec::with_capacity(5);
    for _ in 0..5 {
        let mut samples = Vec::with_capacity(80);
        for _ in 0..80 {
            let start = Instant::now();
            let _ = search(&items, "q4 reort", 20);
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        batch_p95.push(p95_ms(&mut samples));
    }

    batch_p95.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_p95 = batch_p95[batch_p95.len() / 2];

    assert!(
        median_p95 <= 15.0,
        "median batch p95 too high: {median_p95:.3}ms (budget 15.0ms); batches={batch_p95:?}",
    );
}
