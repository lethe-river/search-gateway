use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::circuit::CircuitBreaker;
use crate::config::{CircuitConfig, FirecrawlConfig};
use crate::keyring::Keyring;
use crate::model::{BreakerSnapshot, ErrorCode, ProviderError, Source};

use super::http::KeyedHttp;
use super::{ProviderOutput, ProviderRequest};

pub struct FirecrawlClient {
    search_http: KeyedHttp,
    scrape_http: KeyedHttp,
    map_http: KeyedHttp,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    #[serde(default)]
    success: Option<bool>,
    data: Option<SearchData>,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    #[serde(default)]
    web: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    title: Option<String>,
    description: Option<String>,
    url: Option<String>,
    #[serde(default)]
    published_date: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date_camel: Option<String>,
    #[serde(default)]
    date: Option<String>,
    metadata: Option<Value>,
}

#[derive(Debug)]
pub struct ScrapedPage {
    pub title: Option<String>,
    pub markdown: String,
    pub published_date: Option<String>,
}

impl FirecrawlClient {
    pub fn new(
        cfg: &FirecrawlConfig,
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
            scrape_http: KeyedHttp::new(
                http.clone(),
                keyring.clone(),
                CircuitBreaker::new(circuit_cfg.clone()),
            ),
            map_http: KeyedHttp::new(http, keyring, CircuitBreaker::new(circuit_cfg)),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
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
            "limit": max_results.min(100),
            "sources": ["web"],
        });
        if !request.include_domains.is_empty() {
            body["includeDomains"] = json!(request.include_domains);
        }
        if !request.exclude_domains.is_empty() {
            body["excludeDomains"] = json!(request.exclude_domains);
        }
        if let Some(tbs) = firecrawl_tbs(request.recency_days) {
            body["tbs"] = json!(tbs);
        }

        let url = format!("{}/v2/search", self.base_url);
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

        let parsed: SearchEnvelope = serde_json::from_value(value).map_err(|error| {
            self.search_http.breaker.record_transient_failure();
            ProviderError::new(
                ErrorCode::InvalidResponse,
                format!("unexpected Firecrawl search response: {error}"),
                0,
            )
        })?;
        if parsed.success == Some(false) {
            self.search_http.breaker.record_transient_failure();
            return Err(ProviderError::new(
                ErrorCode::InvalidResponse,
                "Firecrawl search reported success=false",
                0,
            ));
        }
        self.search_http.breaker.record_success();

        let results = parsed.data.map(|data| data.web).unwrap_or_default();
        let raw_count = results.len();
        let sources = results
            .into_iter()
            .filter_map(|result| {
                let published = result
                    .published_date
                    .or(result.published_date_camel)
                    .or(result.date)
                    .or_else(|| metadata_date(result.metadata.as_ref()));
                Some(Source {
                    url: result.url?,
                    provider: "firecrawl".into(),
                    title: result.title.unwrap_or_default(),
                    description: result.description,
                    published_date: published,
                    content: None,
                })
            })
            .collect();
        Ok(ProviderOutput {
            sources,
            provider: "firecrawl",
            raw_count,
            unresponsive_engines: Vec::new(),
        })
    }

    pub async fn scrape(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<ScrapedPage, ProviderError> {
        let endpoint = format!("{}/v2/scrape", self.base_url);
        let body = json!({
            "url": url,
            "formats": ["markdown"],
            "onlyMainContent": true,
        });
        let value = self
            .scrape_http
            .send_json(
                |key| {
                    self.scrape_http
                        .http
                        .post(&endpoint)
                        .bearer_auth(key)
                        .json(&body)
                },
                timeout,
            )
            .await?;

        let envelope: Value = value;
        if envelope.get("success").and_then(Value::as_bool) == Some(false) {
            self.scrape_http.breaker.record_transient_failure();
            return Err(ProviderError::new(
                ErrorCode::InvalidResponse,
                "Firecrawl scrape reported success=false",
                0,
            ));
        }
        let data = envelope.get("data").unwrap_or(&envelope);
        let markdown = data
            .get("markdown")
            .and_then(Value::as_str)
            .or_else(|| data.get("content").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        if markdown.trim().is_empty() {
            self.scrape_http.breaker.record_transient_failure();
            return Err(ProviderError::new(
                ErrorCode::InvalidResponse,
                "Firecrawl scrape returned empty markdown",
                0,
            ));
        }
        let metadata = data.get("metadata");
        let title = metadata
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let published_date = metadata_date(metadata);
        self.scrape_http.breaker.record_success();
        Ok(ScrapedPage {
            title,
            markdown,
            published_date,
        })
    }

    pub async fn map(
        &self,
        base_url: &str,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<String>, ProviderError> {
        let endpoint = format!("{}/v2/map", self.base_url);
        let body = json!({
            "url": base_url,
            "limit": limit,
            "includeSubdomains": true,
        });
        let value = self
            .map_http
            .send_json(
                |key| {
                    self.map_http
                        .http
                        .post(&endpoint)
                        .bearer_auth(key)
                        .json(&body)
                },
                timeout,
            )
            .await?;
        if value.get("success").and_then(Value::as_bool) == Some(false) {
            self.map_http.breaker.record_transient_failure();
            return Err(ProviderError::new(
                ErrorCode::InvalidResponse,
                "Firecrawl map reported success=false",
                0,
            ));
        }
        let links = value
            .get("links")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let urls = links
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
                "firecrawl.search".to_string(),
                self.search_http.breaker_snapshot(),
            ),
            (
                "firecrawl.scrape".to_string(),
                self.scrape_http.breaker_snapshot(),
            ),
            (
                "firecrawl.map".to_string(),
                self.map_http.breaker_snapshot(),
            ),
        ])
    }
}

fn firecrawl_tbs(recency_days: Option<u32>) -> Option<&'static str> {
    match recency_days? {
        0..=1 => Some("qdr:d"),
        2..=7 => Some("qdr:w"),
        8..=31 => Some("qdr:m"),
        32..=365 => Some("qdr:y"),
        _ => None,
    }
}

fn metadata_date(metadata: Option<&Value>) -> Option<String> {
    let metadata = metadata?;
    for key in [
        "publishedTime",
        "published_date",
        "publishedDate",
        "date",
        "modifiedTime",
    ] {
        if let Some(value) = metadata.get(key).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}
