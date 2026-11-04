use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use url::Url;

use crate::budget::Budget;
use crate::config::Config;
use crate::model::{ErrorCode, MapResponse, ProviderError};
use crate::normalize::canonical_url;
use crate::providers::{FirecrawlClient, TavilyClient};
use crate::rate::RateLimiter;

pub struct MapEngine {
    cfg: Arc<Config>,
    tavily: Option<Arc<TavilyClient>>,
    firecrawl: Option<Arc<FirecrawlClient>>,
    limiter: Arc<RateLimiter>,
}

impl MapEngine {
    pub fn new(
        cfg: Arc<Config>,
        tavily: Option<Arc<TavilyClient>>,
        firecrawl: Option<Arc<FirecrawlClient>>,
        limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            cfg,
            tavily,
            firecrawl,
            limiter,
        }
    }

    pub async fn map(&self, raw_url: &str, max_results: usize) -> MapResponse {
        let deadline = Budget::new(Duration::from_millis(self.cfg.map.budget_ms));
        let mut errors: BTreeMap<String, ProviderError> = BTreeMap::new();
        let max_results = max_results.clamp(1, self.cfg.map.max_results_limit);

        // A provider that answered successfully is recorded even when it
        // returned zero URLs, so callers can distinguish that from total
        // provider failure.
        let mut succeeded: Option<&'static str> = None;
        let mut base_url = raw_url.to_string();
        if let Ok(url) = Url::parse(raw_url) {
            if let Some(mut normalized) = canonical_url(url.as_str()).and_then(|value| Url::parse(&value).ok()) {
                normalized.set_query(None);
                normalized.set_fragment(None);
                base_url = normalized.to_string();
            }
        }

        if let Err(retry_after) = self.limiter.acquire() {
            errors.insert(
                "local".into(),
                ProviderError::new(
                    ErrorCode::RateLimited,
                    format!(
                        "local tool rate limit exceeded; retry in {retry_after}s"
                    ),
                    0,
                ),
            );
            return MapResponse {
                base_url,
                urls: Vec::new(),
                provider: None,
                truncated: false,
                omitted_urls: 0,
                provider_errors: errors,
            };
        }

        if let Some(client) = &self.tavily {
            let slice =
                deadline.slice(Duration::from_millis(self.cfg.map.tavily_timeout_ms));
            if slice.is_zero() {
                errors.insert(
                    "tavily".into(),
                    ProviderError::new(
                        ErrorCode::BudgetExhausted,
                        "map deadline exhausted before Tavily",
                        0,
                    ),
                );
            } else {
                match client.map(&base_url, max_results, slice).await {
                    Ok(urls) if !urls.is_empty() => {
                        return build_response(&base_url, urls, "tavily", max_results, errors);
                    }
                    Ok(_) => {
                        succeeded = Some("tavily");
                        errors.insert(
                            "tavily".into(),
                            ProviderError::new(
                                ErrorCode::ZeroResults,
                                "Tavily map returned zero URLs",
                                1,
                            ),
                        );
                    }
                    Err(error) => {
                        errors.insert("tavily".into(), error);
                    }
                }
            }
        }

        if let Some(client) = &self.firecrawl {
            let slice = deadline.slice(Duration::from_millis(
                self.cfg.map.firecrawl_timeout_ms,
            ));
            if slice.is_zero() {
                errors.insert(
                    "firecrawl".into(),
                    ProviderError::new(
                        ErrorCode::BudgetExhausted,
                        "map deadline exhausted before Firecrawl",
                        0,
                    ),
                );
            } else {
                match client.map(&base_url, max_results, slice).await {
                    Ok(urls) if !urls.is_empty() => {
                        return build_response(
                            &base_url,
                            urls,
                            "firecrawl",
                            max_results,
                            errors,
                        );
                    }
                    Ok(_) => {
                        succeeded = Some("firecrawl");
                        errors.insert(
                            "firecrawl".into(),
                            ProviderError::new(
                                ErrorCode::ZeroResults,
                                "Firecrawl map returned zero URLs",
                                1,
                            ),
                        );
                    }
                    Err(error) => {
                        errors.insert("firecrawl".into(), error);
                    }
                }
            }
        }

        if self.tavily.is_none() && self.firecrawl.is_none() {
            errors.insert(
                "map".into(),
                ProviderError::new(
                    ErrorCode::Misconfigured,
                    "web_map requires Tavily or Firecrawl; neither is configured",
                    0,
                ),
            );
        }

        MapResponse {
            base_url,
            urls: Vec::new(),
            provider: succeeded.map(ToString::to_string),
            truncated: false,
            omitted_urls: 0,
            provider_errors: errors,
        }
    }
}

fn build_response(
    base_url: &str,
    raw_urls: Vec<String>,
    provider: &str,
    max_results: usize,
    provider_errors: BTreeMap<String, ProviderError>,
) -> MapResponse {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for raw in raw_urls {
        let Some(url) = canonical_url(&raw) else {
            continue;
        };
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    let truncated = urls.len() > max_results;
    urls.truncate(max_results);
    MapResponse {
        base_url: base_url.to_string(),
        urls,
        provider: Some(provider.to_string()),
        truncated,
        omitted_urls: 0,
        provider_errors,
    }
}
