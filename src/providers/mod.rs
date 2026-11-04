use std::time::Duration;

use crate::model::Source;

pub mod firecrawl;
pub mod http;
pub mod searxng;
pub mod tavily;

pub use firecrawl::FirecrawlClient;
pub use searxng::SearxngClient;
pub use tavily::TavilyClient;

/// Input shared by all search providers.
#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub query: String,
    pub max_results: usize,
    pub recency_days: Option<u32>,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    pub categories: Vec<String>,
}

/// Normalized provider output before cross-provider deduplication.
#[derive(Debug)]
pub struct ProviderOutput {
    pub sources: Vec<Source>,
    pub provider: &'static str,
    pub raw_count: usize,
    /// Engine names reported as unresponsive by the provider, when supported.
    pub unresponsive_engines: Vec<String>,
}

pub fn new_http_client(connect_timeout_ms: u64) -> Result<reqwest::Client, crate::error::GatewayError> {
    reqwest::Client::builder()
        .user_agent(format!(
            "search-gateway/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_millis(connect_timeout_ms))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(crate::error::GatewayError::Http)
}

/// Fetch client with redirects disabled so every hop can be SSRF-checked.
pub fn new_fetch_client(connect_timeout_ms: u64) -> Result<reqwest::Client, crate::error::GatewayError> {
    reqwest::Client::builder()
        .user_agent(format!(
            "search-gateway/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_millis(connect_timeout_ms))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(crate::error::GatewayError::Http)
}
