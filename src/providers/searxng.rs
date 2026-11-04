use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::circuit::CircuitBreaker;
use crate::config::{CircuitConfig, SearxngConfig};
use crate::model::{ErrorCode, ProviderError, Source};

use super::{ProviderOutput, ProviderRequest};

pub struct SearxngClient {
    http: Client,
    base_url: String,
    cfg: SearxngConfig,
    breaker: CircuitBreaker,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
    #[serde(default)]
    unresponsive_engines: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    url: Option<String>,
    title: Option<String>,
    content: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
    #[serde(default)]
    pubdate: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearxConfig {
    #[serde(default)]
    pub engines: Vec<SearxEngine>,
}

#[derive(Debug, Deserialize)]
pub struct SearxEngine {
    pub name: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl SearxngClient {
    pub fn new(cfg: &SearxngConfig, circuit_cfg: CircuitConfig, http: Client) -> Self {
        Self {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            cfg: cfg.clone(),
            breaker: CircuitBreaker::new(circuit_cfg),
        }
    }

    pub async fn search(
        &self,
        request: &ProviderRequest,
        max_results: usize,
        timeout: Duration,
    ) -> Result<ProviderOutput, ProviderError> {
        if !self.breaker.allow() {
            return Err(ProviderError::new(
                ErrorCode::Unreachable,
                "SearXNG circuit breaker is open",
                0,
            ));
        }

        let mut url = Url::parse(&format!("{}/search", self.base_url)).map_err(|error| {
            ProviderError::new(
                ErrorCode::Misconfigured,
                format!("invalid SearXNG base URL: {error}"),
                0,
            )
        })?;
        let search_query =
            searx_query(&request.query, &request.include_domains, &request.exclude_domains);
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("q", &search_query)
                .append_pair("format", "json")
                .append_pair("pageno", "1")
                .append_pair("safesearch", "0")
                .append_pair("timeout_limit", &self.cfg.server_timeout_s.to_string());
            let categories = if request.categories.is_empty() {
                &self.cfg.categories
            } else {
                &request.categories
            };
            if !categories.is_empty() {
                query.append_pair("categories", &categories.join(","));
            }
            let language = self.language(&request.query);
            if !language.is_empty() {
                query.append_pair("language", &language);
            }
            if let Some(range) = searx_time_range(request.recency_days) {
                query.append_pair("time_range", range);
            }
        }

        let timeout = timeout.min(Duration::from_millis(self.cfg.request_timeout_ms));
        let response = match self.http.get(url).timeout(timeout).send().await {
            Ok(response) => response,
            Err(error) => {
                self.breaker.record_transient_failure();
                let code = if error.is_timeout() {
                    ErrorCode::Timeout
                } else {
                    ErrorCode::Unreachable
                };
                return Err(ProviderError::new(code, error.to_string(), 1));
            }
        };

        let status = response.status();
        let retry_after_secs = super::http::retry_after_secs(response.headers());
        if !status.is_success() {
            let code = match status.as_u16() {
                400 | 403 => ErrorCode::Misconfigured,
                429 | 500..=599 => ErrorCode::Unreachable,
                _ => ErrorCode::Misconfigured,
            };
            if code == ErrorCode::Misconfigured {
                self.breaker.record_misconfig();
            } else {
                self.breaker.record_transient_failure();
            }
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::new(
                code,
                format!(
                    "SearXNG HTTP {}: {}",
                    status.as_u16(),
                    super::http::truncate(&body, 300)
                ),
                1,
            )
            .with_retry_after(retry_after_secs));
        }

        let value: Value = match response.json().await {
            Ok(value) => value,
            Err(error) => {
                self.breaker.record_transient_failure();
                return Err(ProviderError::new(
                    ErrorCode::InvalidResponse,
                    format!("SearXNG returned invalid JSON: {error}"),
                    1,
                ));
            }
        };
        let parsed: SearchResponse = match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.breaker.record_transient_failure();
                return Err(ProviderError::new(
                    ErrorCode::InvalidResponse,
                    format!("unexpected SearXNG response: {error}"),
                    1,
                ));
            }
        };
        self.breaker.record_success();

        if !parsed.unresponsive_engines.is_empty() {
            tracing::warn!(
                engines = ?parsed.unresponsive_engines,
                "SearXNG reported unresponsive engines"
            );
        }
        let unresponsive_engines = engine_names(&parsed.unresponsive_engines);

        let raw_count = parsed.results.len();
        let sources = parsed
            .results
            .into_iter()
            .filter_map(|result| {
                Some(Source {
                    url: result.url?,
                    provider: "searxng".into(),
                    title: result.title.unwrap_or_default(),
                    description: result.content,
                    published_date: result.published_date.or(result.pubdate),
                    content: None,
                })
            })
            .take(max_results.min(self.cfg.max_results))
            .collect();
        Ok(ProviderOutput {
            sources,
            provider: "searxng",
            raw_count,
            unresponsive_engines,
        })
    }

    fn language(&self, query: &str) -> String {
        if self.cfg.language != "auto" {
            return self.cfg.language.clone();
        }
        let has_cjk = query
            .chars()
            .any(|c| matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'));
        if has_cjk {
            "zh-CN".into()
        } else {
            "en".into()
        }
    }

    pub async fn health(&self) -> Result<Duration, ProviderError> {
        let started = std::time::Instant::now();
        let url = format!("{}/healthz", self.base_url);
        let response = self
            .http
            .get(url)
            .timeout(Duration::from_millis(self.cfg.connect_timeout_ms.max(1_000)))
            .send()
            .await
            .map_err(|error| {
                ProviderError::new(
                    if error.is_timeout() {
                        ErrorCode::Timeout
                    } else {
                        ErrorCode::Unreachable
                    },
                    error.to_string(),
                    1,
                )
            })?;
        if !response.status().is_success() {
            return Err(ProviderError::new(
                ErrorCode::Unreachable,
                format!("healthz returned HTTP {}", response.status().as_u16()),
                1,
            ));
        }
        Ok(started.elapsed())
    }

    pub async fn config(&self) -> Result<SearxConfig, ProviderError> {
        let url = format!("{}/config", self.base_url);
        let response = self
            .http
            .get(url)
            .timeout(Duration::from_millis(self.cfg.request_timeout_ms))
            .send()
            .await
            .map_err(|error| {
                ProviderError::new(
                    if error.is_timeout() {
                        ErrorCode::Timeout
                    } else {
                        ErrorCode::Unreachable
                    },
                    error.to_string(),
                    1,
                )
            })?;
        if !response.status().is_success() {
            return Err(ProviderError::new(
                ErrorCode::Unreachable,
                format!("/config returned HTTP {}", response.status().as_u16()),
                1,
            ));
        }
        response.json().await.map_err(|error| {
            ProviderError::new(
                ErrorCode::InvalidResponse,
                format!("invalid /config response: {error}"),
                1,
            )
        })
    }

    pub async fn doctor_search(&self) -> Result<ProviderOutput, ProviderError> {
        let request = ProviderRequest {
            query: "searxng doctor".into(),
            max_results: 5,
            recency_days: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            categories: vec!["general".into()],
        };
        self.search(
            &request,
            5,
            Duration::from_millis(self.cfg.request_timeout_ms),
        )
        .await
    }

    pub fn breaker_snapshot(&self) -> crate::model::BreakerSnapshot {
        self.breaker.snapshot()
    }
}

/// SearXNG reports `[["engine", "reason"], ...]`; tolerate plain strings too.
fn engine_names(engines: &[Value]) -> Vec<String> {
    engines
        .iter()
        .filter_map(|value| {
            if let Some(items) = value.as_array() {
                let name = items.first()?.as_str()?;
                let reason = items.get(1).and_then(Value::as_str);
                Some(match reason {
                    Some(reason) if !reason.is_empty() => format!("{name} ({reason})"),
                    _ => name.to_string(),
                })
            } else {
                value.as_str().map(ToString::to_string)
            }
        })
        .collect()
}

fn searx_query(query: &str, include: &[String], exclude: &[String]) -> String {
    let mut output = query.trim().to_string();
    for domain in include {
        let domain = domain.trim();
        if !domain.is_empty() {
            output.push_str(" site:");
            output.push_str(domain);
        }
    }
    for domain in exclude {
        let domain = domain.trim();
        if !domain.is_empty() {
            output.push_str(" -site:");
            output.push_str(domain);
        }
    }
    output
}

fn searx_time_range(recency_days: Option<u32>) -> Option<&'static str> {
    match recency_days? {
        0..=1 => Some("day"),
        2..=7 => Some("week"),
        8..=31 => Some("month"),
        32..=365 => Some("year"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_site_filters_to_the_searx_query() {
        let include = vec!["example.com".to_string()];
        let exclude = vec!["spam.example.com".to_string()];
        assert_eq!(
            searx_query("rust", &include, &exclude),
            "rust site:example.com -site:spam.example.com"
        );
    }
}
