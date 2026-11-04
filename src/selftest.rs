use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::search::SearchParams;

#[derive(Debug, Deserialize)]
struct GoldenSet {
    queries: Vec<GoldenQuery>,
}

#[derive(Debug, Deserialize)]
struct GoldenQuery {
    id: u32,
    language: String,
    categories: Vec<String>,
    query: String,
}

#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub id: u32,
    pub language: String,
    pub categories: Vec<String>,
    pub query: String,
    pub source_count: usize,
    pub elapsed_ms: u64,
    pub ok: bool,
}

#[derive(Debug, Serialize)]
pub struct SelfTestReport {
    pub live_providers: bool,
    pub queries: Vec<QueryResult>,
    pub p95_ms: u64,
    pub passed: bool,
}

pub async fn run(app: &App, live_providers: bool) -> SelfTestReport {
    let golden: GoldenSet =
        toml::from_str(include_str!("../tests/golden_queries.toml")).expect("golden set is valid");
    let mut results = Vec::new();

    for item in golden.queries {
        let started = std::time::Instant::now();
        let response = app
            .search
            .search(SearchParams {
                query: item.query.clone(),
                max_results: 10,
                recency_days: None,
                include_domains: Vec::new(),
                exclude_domains: Vec::new(),
                categories: item.categories.clone(),
                snippet_max_chars: 800,
                fresh: false,
            })
            .await;
        let source_count = response.sources.len();
        results.push(QueryResult {
            id: item.id,
            language: item.language,
            categories: item.categories,
            query: item.query,
            source_count,
            elapsed_ms: started.elapsed().as_millis() as u64,
            ok: source_count >= 3,
        });
    }

    let mut durations: Vec<u64> = results.iter().map(|result| result.elapsed_ms).collect();
    durations.sort_unstable();
    let p95_ms = if durations.is_empty() {
        0
    } else {
        let index = ((durations.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(durations.len() - 1);
        durations[index]
    };
    let passed = !results.is_empty() && results.iter().all(|result| result.ok) && p95_ms < 8_000;

    SelfTestReport {
        live_providers,
        queries: results,
        p95_ms,
        passed,
    }
}
