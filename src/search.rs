use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::cache::SearchCache;
use crate::config::Config;
use crate::model::{
    ErrorCode, ProviderError, SearchMeta, SearchResponse, Source,
};
use crate::normalize::{normalize_sources, recency_cutoff, truncate_chars};
use crate::providers::{
    FirecrawlClient, ProviderRequest, SearxngClient, TavilyClient,
};
use crate::rate::RateLimiter;

#[derive(Debug, Clone)]
pub struct SearchParams {
    pub query: String,
    pub max_results: usize,
    pub recency_days: Option<u32>,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    pub categories: Vec<String>,
    pub snippet_max_chars: usize,
    /// Bypass the in-process query cache for this call.
    pub fresh: bool,
}

pub struct SearchEngine {
    cfg: Arc<Config>,
    tavily: Option<Arc<TavilyClient>>,
    firecrawl: Option<Arc<FirecrawlClient>>,
    searxng: Option<Arc<SearxngClient>>,
    cache: Arc<SearchCache>,
    limiter: Arc<RateLimiter>,
}

impl SearchEngine {
    pub fn new(
        cfg: Arc<Config>,
        tavily: Option<Arc<TavilyClient>>,
        firecrawl: Option<Arc<FirecrawlClient>>,
        searxng: Option<Arc<SearxngClient>>,
        cache: Arc<SearchCache>,
        limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            cfg,
            tavily,
            firecrawl,
            searxng,
            cache,
            limiter,
        }
    }

    pub fn cache(&self) -> &Arc<SearchCache> {
        &self.cache
    }

    pub async fn search(&self, params: SearchParams) -> SearchResponse {
        let started = Instant::now();
        let deadline = crate::budget::Budget::new(Duration::from_millis(
            self.cfg.search.budget_ms,
        ));
        let request_id = Uuid::new_v4().to_string();
        let target = params.max_results.max(1);
        let snippet_cap = params.snippet_max_chars.clamp(100, 2_000);
        let cache_key = query_cache_key(&params);

        if !params.fresh {
            if let Some(cached) = self.cache.get_query(&cache_key) {
                let total_sources = cached.sources.len();
                let search_id = if self.cache.contains(&cached.search_id) {
                    // Keep the source-set entry stable, but remember the cap
                    // used by the most recent request that returned this id.
                    self.cache.set_snippet_cap(&cached.search_id, snippet_cap);
                    cached.search_id
                } else {
                    let id = self.cache.put_with_meta(
                        cached.sources.clone(),
                        snippet_cap,
                        params.query.clone(),
                    );
                    self.cache.replace_query_search_id(&cache_key, id.clone());
                    id
                };
                let mut sources: Vec<Source> =
                    cached.sources.into_iter().take(target).collect();
                trim_descriptions(&mut sources, snippet_cap);
                let mut response = SearchResponse {
                    query: params.query,
                    sources,
                    provider_errors: BTreeMap::new(),
                    meta: SearchMeta {
                        elapsed_ms: 0,
                        chain_used: vec!["cache".into()],
                        budget_exhausted: false,
                        result_count: 0,
                        omitted_sources: 0,
                        truncated: false,
                        snippet_only: true,
                        cached: true,
                        cache_age_secs: cached.age_secs,
                        retry_after_secs: None,
                        search_id,
                        request_id,
                    },
                };
                response.meta.result_count = response.sources.len();
                response.meta.omitted_sources =
                    total_sources.saturating_sub(response.sources.len());
                apply_response_budget(&mut response, self.cfg.search.response_max_chars);
                return response;
            }
        }

        if let Err(retry_after) = self.limiter.acquire() {
            let mut provider_errors = BTreeMap::new();
            provider_errors.insert(
                "local".into(),
                ProviderError::new(
                    ErrorCode::RateLimited,
                    format!(
                        "local tool rate limit exceeded; retry in {retry_after}s"
                    ),
                    0,
                )
                .with_retry_after(Some(retry_after)),
            );
            return SearchResponse {
                query: params.query,
                sources: Vec::new(),
                provider_errors,
                meta: SearchMeta {
                    elapsed_ms: 0,
                    chain_used: vec!["rate_limited".into()],
                    budget_exhausted: false,
                    result_count: 0,
                    omitted_sources: 0,
                    truncated: false,
                    snippet_only: true,
                    cached: false,
                    cache_age_secs: 0,
                    retry_after_secs: Some(retry_after),
                    search_id: String::new(),
                    request_id,
                },
            };
        }

        let provider_request = ProviderRequest {
            query: params.query.clone(),
            max_results: target,
            recency_days: params.recency_days,
            include_domains: params.include_domains.clone(),
            exclude_domains: params.exclude_domains.clone(),
            categories: params.categories.clone(),
        };
        let cutoff = recency_cutoff(params.recency_days);

        let mut errors: BTreeMap<String, ProviderError> = BTreeMap::new();
        let mut chain_used: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut sources: Vec<Source> = Vec::new();
        let mut budget_exhausted = false;
        // Paid stages must leave enough time for the SearXNG last resort.
        let reserve = if self.searxng.is_some() {
            Duration::from_millis(self.cfg.search.reserve_last_resort_ms)
        } else {
            Duration::ZERO
        };

        if let Some(client) = &self.tavily {
            chain_used.push("tavily".into());
            let slice = deadline.slice_reserving(
                Duration::from_millis(self.cfg.tavily.timeout_ms),
                reserve,
            );
            if slice.is_zero() {
                budget_exhausted = true;
                errors.insert(
                    "tavily".into(),
                    ProviderError::new(
                        ErrorCode::BudgetExhausted,
                        "search deadline exhausted before Tavily",
                        0,
                    ),
                );
            } else {
                match client.search(&provider_request, target.min(20), slice).await {
                    Ok(output) => {
                        let normalized = normalize_sources(
                            output.sources,
                            &params.include_domains,
                            &params.exclude_domains,
                            &mut seen,
                            cutoff,
                        );
                        sources.extend(normalized);
                    }
                    Err(error) => {
                        errors.insert("tavily".into(), error);
                    }
                }
            }
        }

        if let Some(client) = &self.firecrawl {
            if sources.len() < target {
                chain_used.push("firecrawl".into());
                let remaining = target - sources.len();
                let slice = deadline.slice_reserving(
                    Duration::from_millis(self.cfg.firecrawl.timeout_ms),
                    reserve,
                );
                if slice.is_zero() {
                    budget_exhausted = true;
                    errors.insert(
                        "firecrawl".into(),
                        ProviderError::new(
                            ErrorCode::BudgetExhausted,
                            "search deadline exhausted before Firecrawl",
                            0,
                        ),
                    );
                } else {
                    match client
                        .search(&provider_request, remaining.min(100), slice)
                        .await
                    {
                        Ok(output) => {
                            let normalized = normalize_sources(
                                output.sources,
                                &params.include_domains,
                                &params.exclude_domains,
                                &mut seen,
                                cutoff,
                            );
                            sources.extend(normalized);
                        }
                        Err(error) => {
                            errors.insert("firecrawl".into(), error);
                        }
                    }
                }
            }
        }

        if sources.is_empty() {
            if let Some(client) = &self.searxng {
                chain_used.push("searxng".into());
                let slice = deadline.slice(Duration::from_millis(
                    self.cfg.searxng.request_timeout_ms,
                ));
                let minimum = Duration::from_millis(self.cfg.search.reserve_last_resort_ms)
                    .min(Duration::from_millis(self.cfg.searxng.request_timeout_ms));
                if slice < minimum {
                    budget_exhausted = true;
                    errors.insert(
                        "searxng".into(),
                        ProviderError::new(
                            ErrorCode::BudgetExhausted,
                            format!(
                                "less than {}ms of search budget remains",
                                self.cfg.search.reserve_last_resort_ms
                            ),
                            0,
                        ),
                    );
                } else {
                    match client
                        .search(
                            &provider_request,
                            target.min(self.cfg.searxng.max_results),
                            slice,
                        )
                        .await
                    {
                        Ok(output) => {
                            let engines = output.unresponsive_engines;
                            let normalized = normalize_sources(
                                output.sources,
                                &params.include_domains,
                                &params.exclude_domains,
                                &mut seen,
                                cutoff,
                            );
                            sources.extend(normalized);
                            if sources.is_empty() {
                                let message = if engines.is_empty() {
                                    "SearXNG returned zero normalized sources".to_string()
                                } else {
                                    format!(
                                        "SearXNG returned zero normalized sources; unresponsive engines: {}",
                                        engines.join(", ")
                                    )
                                };
                                errors.insert(
                                    "searxng".into(),
                                    ProviderError::new(ErrorCode::ZeroResults, message, 1),
                                );
                            }
                        }
                        Err(error) => {
                            errors.insert("searxng".into(), error);
                        }
                    }
                }
            }
        }

        if sources.is_empty() && errors.is_empty() {
            errors.insert(
                "chain".into(),
                ProviderError::new(
                    ErrorCode::ZeroResults,
                    "all providers returned zero normalized sources",
                    0,
                ),
            );
        }

        let retry_after_secs = if sources.is_empty() {
            errors
                .values()
                .filter_map(|error| error.retry_after_secs)
                .max()
        } else {
            None
        };
        let full_sources = sources.clone();
        let total_sources = full_sources.len();
        let search_id = self.cache.put_with_meta(
            full_sources.clone(),
            params.snippet_max_chars,
            params.query.clone(),
        );
        if errors.is_empty() {
            self.cache.put_query(
                cache_key,
                full_sources.clone(),
                chain_used.clone(),
                search_id.clone(),
            );
        }
        let mut selected: Vec<Source> = full_sources.into_iter().take(target).collect();
        trim_descriptions(&mut selected, snippet_cap);

        let mut response = SearchResponse {
            query: params.query,
            sources: selected,
            provider_errors: errors,
            meta: SearchMeta {
                elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                chain_used,
                budget_exhausted,
                result_count: 0,
                omitted_sources: 0,
                truncated: false,
                snippet_only: true,
                cached: false,
                cache_age_secs: 0,
                retry_after_secs,
                search_id,
                request_id,
            },
        };
        response.meta.result_count = response.sources.len();
        response.meta.omitted_sources =
            total_sources.saturating_sub(response.sources.len());
        apply_response_budget(&mut response, self.cfg.search.response_max_chars);
        response
    }
}

fn trim_descriptions(sources: &mut [Source], max_chars: usize) {
    for source in sources {
        if let Some(description) = &source.description {
            source.description = Some(truncate_chars(description, max_chars).0);
        }
    }
}

fn apply_response_budget(response: &mut SearchResponse, budget: usize) {
    while response_chars(response) > budget {
        if response.sources.len() > 1 {
            response.sources.pop();
            response.meta.omitted_sources += 1;
            response.meta.truncated = true;
        } else if let Some(source) = response.sources.last_mut() {
            let Some(description) = &source.description else {
                break;
            };
            let current = description.chars().count();
            let next = (current.saturating_mul(3) / 4).max(1);
            if next >= current {
                break;
            }
            source.description = Some(truncate_chars(description, next).0);
            response.meta.truncated = true;
        } else {
            break;
        }
        response.meta.result_count = response.sources.len();
    }
    response.meta.result_count = response.sources.len();
}

fn response_chars(response: &SearchResponse) -> usize {
    serde_json::to_string(response)
        .map(|text| text.chars().count())
        .unwrap_or(usize::MAX)
}

fn query_cache_key(params: &SearchParams) -> String {
    let mut include: Vec<String> = params
        .include_domains
        .iter()
        .map(|domain| domain.trim().to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect();
    include.sort();
    include.dedup();

    let mut exclude: Vec<String> = params
        .exclude_domains
        .iter()
        .map(|domain| domain.trim().to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect();
    exclude.sort();
    exclude.dedup();

    let mut categories = params.categories.clone();
    categories.sort();
    categories.dedup();

    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        params.query.trim(),
        params.max_results,
        params.recency_days
            .map(|days| days.to_string())
            .unwrap_or_default(),
        include.join(","),
        exclude.join(","),
        categories.join(",")
    )
}
