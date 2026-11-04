use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::circuit::CircuitBreaker;
use crate::config::{CircuitConfig, TavilyConfig};
use crate::keyring::Keyring;
use crate::model::{BreakerSnapshot, ErrorCode, ProviderError, Source};

use super::http::KeyedHttp;
use super::{ProviderOutput, ProviderRequest};

pub struct TavilyClient {
    search_http: KeyedHttp,
    map_http: KeyedHttp,
    base_url: String,
    search_depth: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
    published_date: Option<String>,
}

impl TavilyClient {
    pub fn new(
        cfg: &TavilyConfig,
        keyring: Arc<Mutex<Keyring>>,
        circuit_cfg: CircuitConfig,
        http: Client,
    ) -> Self {
        Self {
            search_http: KeyedHttp::new(
                http.clone(),
                keyring.clone(),
                CircuitBreaker::new(circuit_cfg.clone()),
            ),
            map_http: KeyedHttp::new(http, keyring, CircuitBreaker::new(circuit_cfg)),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            search_depth: cfg.search_depth.clone(),
        }
    }

    pub async fn search(
        &self,
        request: &ProviderRequest,
        max_results: usize,
        timeout: Duration,
    ) -> Result<ProviderOutput, ProviderError> {
        let mut body = json!({
            "query": request.query,
            "search_depth": self.search_depth,
            "max_results": max_results.min(20),
            "include_answer": false,
            "include_raw_content": false,
            "include_published_date": true,
        });
        if !request.include_domains.is_empty() {
            body["include_domains"] = json!(request.include_domains);
        }
        if !request.exclude_domains.is_empty() {
            body["exclude_domains"] = json!(request.exclude_domains);
        }
        if let Some(range) = tavily_time_range(request.recency_days) {
            body["time_range"] = json!(range);
        }

        let url = format!("{}/search", self.base_url);
        let value = self
            .search_http
            .send_json(
                |key| {
                    self.search_http
                        .http
                        .post(&url)
                        .bearer_auth(key)
                        .json(&body)
                },
                timeout,
            )
            .await?;

        let parsed: SearchResponse = serde_json::from_value(value).map_err(|error| {
            self.search_http.breaker.record_transient_failure();
            ProviderError::new(
                ErrorCode::InvalidResponse,
                format!("unexpected Tavily search response: {error}"),
                0,
            )
        })?;
        self.search_http.breaker.record_success();

        let raw_count = parsed.results.len();
        let sources = parsed
            .results
            .into_iter()
            .filter_map(|result| {
                Some(Source {
                    url: result.url?,
                    provider: "tavily".into(),
                    title: result.title.unwrap_or_default(),
                    description: result.content,
                    published_date: result.published_date,
                    content: None,
                })
            })
            .collect();
        Ok(ProviderOutput {
            sources,
            provider: "tavily",
            raw_count,
            unresponsive_engines: Vec::new(),
        })
    }

    pub async fn map(
        &self,
        base_url: &str,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<String>, ProviderError> {
        let url = format!("{}/map", self.base_url);
        let body = json!({
            "url": base_url,
            "limit": limit,
            "allow_external": false,
        });
        let value = self
            .map_http
            .send_json(
                |key| {
                    self.map_http
                        .http
                        .post(&url)
                        .bearer_auth(key)
                        .json(&body)
                },
                timeout,
            )
            .await?;
        let parsed: Value = value;
        let results = parsed
            .get("results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let urls = results
            .iter()
            .filter_map(|item| {
                item.as_str()
                    .map(ToString::to_string)
                    .or_else(|| item.get("url").and_then(Value::as_str).map(ToString::to_string))
            })
            .collect();
        self.map_http.breaker.record_success();
        Ok(urls)
    }

    pub fn keyring_snapshot(&self) -> Vec<crate::model::KeySnapshot> {
        self.search_http.keyring_snapshot()
    }

    pub fn breaker_snapshot(&self) -> BTreeMap<String, BreakerSnapshot> {
        BTreeMap::from([
            (
                "tavily.search".to_string(),
                self.search_http.breaker_snapshot(),
            ),
            ("tavily.map".to_string(), self.map_http.breaker_snapshot()),
        ])
    }
}

fn tavily_time_range(recency_days: Option<u32>) -> Option<&'static str> {
    match recency_days? {
        0..=1 => Some("day"),
        2..=7 => Some("week"),
        8..=31 => Some("month"),
        32..=365 => Some("year"),
        _ => None,
    }
}
