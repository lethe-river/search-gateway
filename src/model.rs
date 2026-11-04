use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Fixed error taxonomy for provider failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unreachable,
    Timeout,
    Http5xx,
    #[serde(rename = "http_4xx")]
    Http4xx,
    InvalidResponse,
    InvalidRequest,
    Auth,
    QuotaExhausted,
    RateLimited,
    AllKeysCooling,
    CircuitOpen,
    Misconfigured,
    Blocked,
    UnsupportedContentType,
    ZeroResults,
    BudgetExhausted,
    UnknownSearchId,
    UnknownFetchId,
}

impl ErrorCode {
    pub fn retryable(self) -> bool {
        !matches!(
            self,
            Self::Auth
                | Self::Misconfigured
                | Self::InvalidRequest
                | Self::Http4xx
                | Self::Blocked
                | Self::UnsupportedContentType
                | Self::ZeroResults
                | Self::UnknownSearchId
                | Self::UnknownFetchId
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProviderError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub attempts: u32,
    /// Provider-supplied delay hint (Retry-After) in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl ProviderError {
    pub fn new(code: ErrorCode, message: impl Into<String>, attempts: u32) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.retryable(),
            attempts,
            retry_after_secs: None,
        }
    }

    pub fn with_retry_after(mut self, retry_after_secs: Option<u64>) -> Self {
        self.retry_after_secs = retry_after_secs;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Source {
    pub url: String,
    pub provider: String,
    pub title: String,
    pub description: Option<String>,
    pub published_date: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchMeta {
    pub elapsed_ms: u64,
    pub chain_used: Vec<String>,
    pub budget_exhausted: bool,
    pub result_count: usize,
    /// Sources from the cached set that are not present in this response.
    /// Retrieve them through `get_sources` with the returned `search_id`.
    pub omitted_sources: usize,
    /// True when the response budget removed sources from the tail.
    pub truncated: bool,
    /// True when returned sources carry only search snippets; call `web_fetch`
    /// before quoting a number or a specific claim.
    pub snippet_only: bool,
    /// True when this response came from the in-process query cache.
    pub cached: bool,
    /// Age of the cached result in seconds; zero for a fresh search.
    pub cache_age_secs: u64,
    /// Longest provider-supplied Retry-After hint when the search returned no
    /// sources. Null when sources were returned or no hint was available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    pub search_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchResponse {
    pub query: String,
    pub sources: Vec<Source>,
    pub provider_errors: BTreeMap<String, ProviderError>,
    pub meta: SearchMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
    /// Provider-supplied delay hint (Retry-After) in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    pub fn from_provider(error: &ProviderError) -> Self {
        Self {
            code: error.code,
            message: error.message.clone(),
            retry_after_secs: error.retry_after_secs,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetSourcesResponse {
    pub search_id: String,
    pub total_sources: usize,
    /// Effective description cap used for this page.
    pub snippet_max_chars: usize,
    pub sources: Vec<Source>,
    pub next_offset: Option<usize>,
    /// True when at least one returned description was shortened by the
    /// snippet cap, or the response budget removed sources from the page.
    pub truncated: bool,
    /// True when the returned sources carry only search snippets; call
    /// `web_fetch` before quoting a number or a specific claim.
    pub snippet_only: bool,
    pub error: Option<ToolError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FetchResponse {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
    pub original_length: usize,
    pub truncated: bool,
    /// True when this response reaches the end of the extracted page.
    pub end_of_content: bool,
    /// HTTP status of the last origin request made for this URL, when one was
    /// made. A 4xx status is also reported through `warning` even when a
    /// Firecrawl fallback produced usable content.
    pub http_status: Option<u16>,
    pub extractor: String,
    /// `ok`, `low`, or `unknown` when the fetch failed.
    pub quality: String,
    pub quality_score: u8,
    /// True when the returned content is suitable to cite directly. False
    /// means the host must disclose `citation_caveat`, or fetch another source.
    pub citation_ready: bool,
    /// Why `citation_ready` is false, or a non-blocking caveat (for example a
    /// truncated page that still has usable content in this chunk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_caveat: Option<String>,
    /// Opaque handle for continuing a truncated page without refetching.
    pub fetch_id: Option<String>,
    pub next_offset: Option<usize>,
    pub fallback_reason: Option<String>,
    /// Non-fatal issue with returned content, for example an origin 4xx that
    /// still produced usable content through a fallback extractor.
    pub warning: Option<ToolError>,
    pub error: Option<ToolError>,
}

impl FetchResponse {
    /// Derive the citation gate from the extraction quality, origin status,
    /// warnings and pagination. This keeps the host from deciding alone whether
    /// a low-quality or partial page is safe to quote.
    pub fn annotate_citation(&mut self) {
        let mut caveats = Vec::new();
        if let Some(error) = &self.error {
            caveats.push(format!("extraction failed: {}", error.message));
        }
        if self.quality == "low" || self.quality_score < 45 {
            caveats.push(format!("low extraction quality ({})", self.quality_score));
        }
        if let Some(warning) = &self.warning {
            caveats.push(format!("origin warning: {}", warning.message));
        } else if self
            .http_status
            .map(|status| (400..500).contains(&status))
            .unwrap_or(false)
        {
            caveats.push(format!(
                "origin returned HTTP {}; content may be an error page",
                self.http_status.unwrap_or_default()
            ));
        }
        if self.error.is_none() && (self.truncated || !self.end_of_content) {
            caveats.push(
                "page truncated; continue with fetch_id before quoting beyond this chunk"
                    .to_string(),
            );
        }
        self.citation_ready = caveats.is_empty();
        self.citation_caveat = (!caveats.is_empty()).then(|| caveats.join("; "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetch_response() -> FetchResponse {
        FetchResponse {
            url: "https://example.com".into(),
            title: Some("Example".into()),
            content: "body".into(),
            original_length: 4,
            truncated: false,
            end_of_content: true,
            http_status: Some(200),
            extractor: "generic".into(),
            quality: "ok".into(),
            quality_score: 90,
            citation_ready: false,
            citation_caveat: None,
            fetch_id: Some("page-id".into()),
            next_offset: None,
            fallback_reason: None,
            warning: None,
            error: None,
        }
    }

    #[test]
    fn good_page_is_citation_ready() {
        let mut response = fetch_response();
        response.annotate_citation();
        assert!(response.citation_ready);
        assert!(response.citation_caveat.is_none());
    }

    #[test]
    fn low_quality_page_is_not_citation_ready() {
        let mut response = fetch_response();
        response.quality = "low".into();
        response.quality_score = 30;
        response.annotate_citation();
        assert!(!response.citation_ready);
        assert!(
            response
                .citation_caveat
                .unwrap()
                .contains("low extraction quality")
        );
    }

    #[test]
    fn http_4xx_warning_is_not_citation_ready() {
        let mut response = fetch_response();
        response.http_status = Some(403);
        response.warning = Some(ToolError::new(
            ErrorCode::Http4xx,
            "origin returned HTTP 403",
        ));
        response.annotate_citation();
        assert!(!response.citation_ready);
        assert!(
            response
                .citation_caveat
                .unwrap()
                .contains("origin warning")
        );
    }

    #[test]
    fn truncated_page_is_not_citation_ready() {
        let mut response = fetch_response();
        response.truncated = true;
        response.end_of_content = false;
        response.next_offset = Some(12000);
        response.annotate_citation();
        assert!(!response.citation_ready);
        assert!(
            response
                .citation_caveat
                .unwrap()
                .contains("page truncated")
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MapResponse {
    pub base_url: String,
    pub urls: Vec<String>,
    pub provider: Option<String>,
    /// True when the response budget removed URLs from the tail.
    pub truncated: bool,
    /// Number of URLs removed from this response by the response budget.
    pub omitted_urls: usize,
    pub provider_errors: BTreeMap<String, ProviderError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Degraded,
    Unreachable,
    Misconfigured,
    ZeroResults,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DoctorOverallStatus {
    Ok,
    Degraded,
    /// Some checks were skipped (for example paid probes without live=true),
    /// so overall health could not be verified.
    Unknown,
    Unreachable,
    Misconfigured,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub latency_ms: Option<u64>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BreakerSnapshot {
    pub state: String,
    pub failures: u32,
    pub open_remaining_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeySnapshot {
    pub index: usize,
    pub healthy: bool,
    pub cooldown_remaining_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DoctorResponse {
    pub status: DoctorOverallStatus,
    /// True when the report was produced with live=true and therefore
    /// includes live probes.
    pub live_verified: bool,
    pub checks: Vec<DoctorCheck>,
    pub breaker: BTreeMap<String, BreakerSnapshot>,
    pub keyring: BTreeMap<String, Vec<KeySnapshot>>,
}
