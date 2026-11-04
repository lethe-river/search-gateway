use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::app::App;
use crate::model::{
    DoctorResponse, ErrorCode, FetchResponse, GetSourcesResponse, MapResponse, SearchResponse,
    ToolError,
};
use crate::normalize::truncate_chars;
use crate::search::SearchParams;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebSearchParams {
    #[schemars(description = "The search query")]
    pub query: String,
    #[schemars(description = "Maximum number of sources to return (1-50, default 5)")]
    pub max_results: Option<usize>,
    #[schemars(
        description = "Only return sources published within this many days (1-365). Providers only support coarse day/week/month/year hints, so sources with a parseable date older than this many days are dropped locally; undated sources may remain."
    )]
    pub recency_days: Option<u32>,
    #[schemars(
        description = "Characters kept from each source description (100-2000; defaults to the server setting)"
    )]
    pub snippet_max_chars: Option<usize>,
    #[schemars(
        description = "Bypass the in-process query cache for this call. Use when the same query must return current results; the default false reuses an identical recent search and reports meta.cached and meta.cache_age_secs."
    )]
    pub fresh: Option<bool>,
    #[schemars(description = "Only include these domains; at most 10 entries")]
    #[serde(default)]
    pub include_domains: Vec<String>,
    #[schemars(description = "Exclude these domains; at most 10 entries")]
    #[serde(default)]
    pub exclude_domains: Vec<String>,
    #[schemars(
        description = "SearXNG categories: general, it, science. Defaults to the [searxng] categories setting, or [\"general\"] when that is empty. Only used when the SearXNG last-resort path runs; Tavily and Firecrawl ignore this field."
    )]
    #[serde(default)]
    pub categories: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebFetchParams {
    #[schemars(description = "Absolute http(s) URL to fetch")]
    pub url: String,
    #[schemars(description = "Maximum characters of extracted content (default 12000, max 200000)")]
    pub max_chars: Option<usize>,
    #[schemars(
        description = "Character offset into the extracted Markdown (default 0). Use next_offset from a previous response to continue."
    )]
    pub offset: Option<usize>,
    #[schemars(
        description = "fetch_id from a previous web_fetch response. When set, the page is read from the local page cache instead of the network. It must be paired with the same url that produced it."
    )]
    pub fetch_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebMapParams {
    #[schemars(description = "Base URL whose site map should be discovered")]
    pub url: String,
    #[schemars(description = "Maximum number of URLs to return (default 50, max 200)")]
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSourcesParams {
    #[schemars(description = "search_id from a previous web_search response")]
    pub search_id: String,
    #[schemars(description = "Offset into the cached source list (default 0)")]
    pub offset: Option<usize>,
    #[schemars(description = "Page size (1-50, default 20)")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Characters kept from each description (100-2000). Defaults to the cap stored with the original search. The response reports the effective value and sets truncated when a description was shortened."
    )]
    pub snippet_max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DoctorParams {
    #[schemars(description = "Send one minimal request to Tavily and Firecrawl (spends credits)")]
    pub live: Option<bool>,
}

#[derive(Clone)]
pub struct GatewayServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    app: Arc<App>,
}

impl GatewayServer {
    pub fn new(app: App) -> Self {
        Self {
            tool_router: Self::tool_router(),
            app: Arc::new(app),
        }
    }
}

#[tool_router]
impl GatewayServer {
    #[tool(
        name = "web_search",
        annotations(title = "Web Search", read_only_hint = true),
        description = "Search the web and return normalized sources (URL, provider, title, snippet, published date). The host writes the answer; this tool does not synthesize one. isError is true only when no sources were returned because of a provider failure (for example auth, timeout, rate limiting or budget exhaustion). A clean zero-result answer (zero_results) is returned with isError=false. Partial provider failures are reported in provider_errors. meta.cached and meta.cache_age_secs describe a reused cached result; pass fresh=true to bypass the cache. meta.snippet_only=true means every source carries only a search snippet; call web_fetch before quoting a number or claim.",
        output_schema = rmcp::handler::server::tool::schema_for_output::<SearchResponse>()
    )]
    async fn web_search(
        &self,
        Parameters(params): Parameters<WebSearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = validate_search(&self.app, params)?;
        let response = self.app.search.search(params).await;
        let is_error = search_is_error(&response);
        Ok(tool_result(&response, is_error))
    }

    #[tool(
        name = "web_fetch",
        annotations(title = "Web Fetch", read_only_hint = true),
        description = "Fetch one URL and return clean Markdown. Uses a specialist extractor, then generic readability extraction, then Firecrawl when the local extraction scores low. A total extraction failure is reported as a tool error. Long pages are paginated: the response carries fetch_id and next_offset; call again with the same url plus fetch_id and offset to read the next chunk without a new network request. offset > 0 requires fetch_id. end_of_content marks the last chunk. http_status reports the last origin HTTP status; a non-fatal 4xx origin is reported through warning while usable content is still returned with isError=false. quality/quality_score report extraction confidence. citation_ready=false is a hard citation gate: if truncated, continue with fetch_id until end_of_content=true or disclose citation_caveat next to every claim; if warning/4xx/low quality, disclose citation_caveat or use another source. Never present content from a citation_ready=false page as fully read.",
        output_schema = rmcp::handler::server::tool::schema_for_output::<FetchResponse>()
    )]
    async fn web_fetch(
        &self,
        Parameters(params): Parameters<WebFetchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        validate_http_url(&params.url)?;
        let limit = self.app.config.fetch.max_chars_limit;
        if let Some(max_chars) = params.max_chars {
            if max_chars == 0 || max_chars > limit {
                return Err(ErrorData::invalid_params(
                    format!("max_chars must be between 1 and {limit}"),
                    None,
                ));
            }
        }
        if let Some(fetch_id) = &params.fetch_id {
            if fetch_id.trim().is_empty() {
                return Err(ErrorData::invalid_params(
                    "fetch_id must not be empty",
                    None,
                ));
            }
        }
        let offset = params.offset.unwrap_or(0);
        if offset > 0 && params.fetch_id.is_none() {
            return Err(ErrorData::invalid_params(
                "offset > 0 requires fetch_id; fetch the URL without offset first",
                None,
            ));
        }
        let mut response = self
            .app
            .fetch
            .fetch_page(
                &params.url,
                params.max_chars,
                offset,
                params.fetch_id.as_deref(),
            )
            .await;
        response.annotate_citation();
        let is_error = response.error.is_some();
        Ok(tool_result(&response, is_error))
    }

    #[tool(
        name = "web_map",
        annotations(title = "Web Map", read_only_hint = true),
        description = "Discover URLs on a website. Tries Tavily Map, then Firecrawl Map. A provider failure that leaves no URLs is reported as a tool error; a successful provider that finds zero URLs is not. The response is capped by map.response_max_chars; truncated and omitted_urls report URLs removed by that cap.",
        output_schema = rmcp::handler::server::tool::schema_for_output::<MapResponse>()
    )]
    async fn web_map(
        &self,
        Parameters(params): Parameters<WebMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        validate_http_url(&params.url)?;
        let limit = self.app.config.map.max_results_limit;
        let max_results = params.max_results.unwrap_or(
            self.app.config.map.default_max_results,
        );
        if !(1..=limit).contains(&max_results) {
            return Err(ErrorData::invalid_params(
                format!("max_results must be between 1 and {limit}"),
                None,
            ));
        }
        let mut response = self.app.map.map(&params.url, max_results).await;
        trim_map_budget(&mut response, self.app.config.map.response_max_chars);
        let is_error = map_is_error(&response);
        Ok(tool_result(&response, is_error))
    }

    #[tool(
        name = "get_sources",
        annotations(title = "Get Sources", read_only_hint = true),
        description = "Page through the full normalized source set cached by a previous web_search. Does not issue a new search. Descriptions use the cap stored by the original search unless snippet_max_chars overrides it; the response reports snippet_max_chars and sets truncated when a description was shortened or the response budget removed sources from the page. snippet_only=true means descriptions only; call web_fetch before quoting. An unknown or expired search_id is reported as a tool error.",
        output_schema = rmcp::handler::server::tool::schema_for_output::<GetSourcesResponse>()
    )]
    async fn get_sources(
        &self,
        Parameters(params): Parameters<GetSourcesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if params.search_id.trim().is_empty() {
            return Err(ErrorData::invalid_params("search_id must not be empty", None));
        }
        let requested_limit = params.limit.unwrap_or(20);
        if !(1..=50).contains(&requested_limit) {
            return Err(ErrorData::invalid_params(
                "limit must be between 1 and 50",
                None,
            ));
        }
        if let Some(cap) = params.snippet_max_chars {
            if !(100..=2_000).contains(&cap) {
                return Err(ErrorData::invalid_params(
                    "snippet_max_chars must be between 100 and 2000",
                    None,
                ));
            }
        }
        let default_cap = self.app.config.search.snippet_max_chars;
        let budget = self.app.config.search.response_max_chars;
        let per_source = params
            .snippet_max_chars
            .unwrap_or(default_cap)
            .saturating_add(512)
            .max(1);
        let limit = requested_limit.min((budget / per_source).max(1));
        let offset = params.offset.unwrap_or(0);

        let response = match self.app.cache.get(&params.search_id, offset, limit) {
            Some(page) => {
                let total_sources = page.total_sources;
                let snippet_cap = params
                    .snippet_max_chars
                    .unwrap_or(page.snippet_max_chars)
                    .clamp(100, 2_000);
                let mut sources = page.sources;
                let mut truncated = false;
                for source in &mut sources {
                    if let Some(description) = &source.description {
                        let (text, was_truncated, _) =
                            truncate_chars(description, snippet_cap);
                        truncated |= was_truncated;
                        source.description = Some(text);
                    }
                }
                let mut response = GetSourcesResponse {
                    search_id: params.search_id,
                    total_sources,
                    snippet_max_chars: snippet_cap,
                    sources,
                    next_offset: None,
                    truncated,
                    snippet_only: true,
                    error: None,
                };
                trim_get_sources_budget(&mut response, offset, budget);
                response
            }
            None => GetSourcesResponse {
                search_id: params.search_id,
                total_sources: 0,
                snippet_max_chars: params.snippet_max_chars.unwrap_or(default_cap),
                sources: Vec::new(),
                next_offset: None,
                truncated: false,
                snippet_only: true,
                error: Some(ToolError::new(
                    ErrorCode::UnknownSearchId,
                    "search_id is unknown or has expired",
                )),
            },
        };
        let is_error = response.error.is_some();
        Ok(tool_result(&response, is_error))
    }

    #[tool(
        name = "doctor",
        annotations(title = "Doctor", read_only_hint = true),
        description = "Check configuration, key rings, circuit breakers, SearXNG health and engine state. Paid provider probes run only with live=true; live=true also runs a free fetch probe of [doctor] fetch_probe_url. Without live, skipped paid probes make the overall status unknown and live_verified is false."
    )]
    async fn doctor(
        &self,
        Parameters(params): Parameters<DoctorParams>,
    ) -> Result<Json<DoctorResponse>, ErrorData> {
        Ok(Json(
            self.app.doctor.run(params.live.unwrap_or(false)).await,
        ))
    }
}

#[tool_handler]
impl ServerHandler for GatewayServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "search-gateway",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Retrieval-only search gateway. It returns normalized sources and never writes an answer. Cite the sources in your own response.",
            )
    }
}

fn tool_result<T: serde::Serialize>(value: &T, is_error: bool) -> CallToolResult {
    let json = serde_json::to_value(value).unwrap_or_else(|error| {
        serde_json::json!({
            "error": {
                "code": "invalid_response",
                "message": error.to_string()
            }
        })
    });
    if is_error {
        CallToolResult::structured_error(json)
    } else {
        CallToolResult::structured(json)
    }
}

fn trim_get_sources_budget(
    response: &mut GetSourcesResponse,
    offset: usize,
    budget: usize,
) {
    while get_sources_chars(response) > budget {
        if response.sources.len() > 1 {
            response.sources.pop();
            response.truncated = true;
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
            response.truncated = true;
        } else {
            break;
        }
    }
    let next = offset.saturating_add(response.sources.len());
    response.next_offset = (next < response.total_sources).then_some(next);
}

/// A clean zero-result answer is a successful search; only an empty answer
/// caused by a real provider failure is a tool error.
fn search_is_error(response: &SearchResponse) -> bool {
    response.sources.is_empty()
        && response
            .provider_errors
            .values()
            .any(|error| error.code != ErrorCode::ZeroResults)
}

/// A provider that answered, even with zero URLs, is a successful map call.
fn map_is_error(response: &MapResponse) -> bool {
    response.provider.is_none()
}

fn trim_map_budget(response: &mut MapResponse, budget: usize) {
    while map_chars(response) > budget && response.urls.len() > 1 {
        response.urls.pop();
        response.omitted_urls += 1;
        response.truncated = true;
    }
}

fn map_chars(response: &MapResponse) -> usize {
    serde_json::to_string(response)
        .map(|text| text.chars().count())
        .unwrap_or(usize::MAX)
}

fn get_sources_chars(response: &GetSourcesResponse) -> usize {
    serde_json::to_string(response)
        .map(|text| text.chars().count())
        .unwrap_or(usize::MAX)
}

fn validate_search(app: &App, params: WebSearchParams) -> Result<SearchParams, ErrorData> {
    let query = params.query.trim().to_string();
    if query.is_empty() {
        return Err(ErrorData::invalid_params("query must not be empty", None));
    }
    let max_results = params
        .max_results
        .unwrap_or(app.config.search.default_max_results);
    if !(1..=50).contains(&max_results) {
        return Err(ErrorData::invalid_params(
            "max_results must be between 1 and 50",
            None,
        ));
    }
    if let Some(days) = params.recency_days {
        if !(1..=365).contains(&days) {
            return Err(ErrorData::invalid_params(
                "recency_days must be between 1 and 365",
                None,
            ));
        }
    }
    let snippet_max_chars = params
        .snippet_max_chars
        .unwrap_or(app.config.search.snippet_max_chars);
    if !(100..=2_000).contains(&snippet_max_chars) {
        return Err(ErrorData::invalid_params(
            "snippet_max_chars must be between 100 and 2000",
            None,
        ));
    }
    for (label, domains) in [
        ("include_domains", &params.include_domains),
        ("exclude_domains", &params.exclude_domains),
    ] {
        if domains.len() > 10 {
            return Err(ErrorData::invalid_params(
                format!("{label} accepts at most 10 entries"),
                None,
            ));
        }
        if domains.iter().any(|domain| {
            domain.trim().is_empty() || domain.chars().any(char::is_whitespace)
        }) {
            return Err(ErrorData::invalid_params(
                format!("{label} entries must be non-empty and contain no whitespace"),
                None,
            ));
        }
    }
    let explicit_categories = !params.categories.is_empty();
    let categories = if explicit_categories {
        params.categories
    } else {
        let configured = &app.config.searxng.categories;
        if configured.is_empty() {
            vec!["general".into()]
        } else {
            configured.clone()
        }
    };
    if explicit_categories {
        for category in &categories {
            if !matches!(category.as_str(), "general" | "it" | "science") {
                return Err(ErrorData::invalid_params(
                    "categories may only contain general, it and science",
                    None,
                ));
            }
        }
    }

    Ok(SearchParams {
        query,
        max_results,
        recency_days: params.recency_days,
        include_domains: params.include_domains,
        exclude_domains: params.exclude_domains,
        categories,
        snippet_max_chars,
        fresh: params.fresh.unwrap_or(false),
    })
}

fn validate_http_url(raw: &str) -> Result<(), ErrorData> {
    match url::Url::parse(raw) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => Ok(()),
        _ => Err(ErrorData::invalid_params(
            "url must be an absolute http(s) URL",
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::config::{Config, LoadedConfig};
    use crate::model::{ProviderError, SearchMeta, Source};

    fn app() -> App {
        App::new_lenient(LoadedConfig {
            config: Config::default(),
            path: None,
            warnings: Vec::new(),
        })
        .expect("app builds")
    }

    fn params(recency_days: Option<u32>) -> WebSearchParams {
        WebSearchParams {
            query: "test".into(),
            max_results: None,
            recency_days,
            snippet_max_chars: None,
            fresh: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn categories_default_to_config_when_not_provided() {
        let mut config = Config::default();
        config.searxng.categories = vec!["it".into()];
        let app = App::new_lenient(LoadedConfig {
            config,
            path: None,
            warnings: Vec::new(),
        })
        .expect("app builds");
        let parsed = validate_search(&app, params(None)).expect("valid search");
        assert_eq!(parsed.categories, vec!["it"]);
    }

    #[test]
    fn accepts_recency_up_to_one_year() {
        let parsed = validate_search(&app(), params(Some(365))).expect("365 is valid");
        assert_eq!(parsed.recency_days, Some(365));
    }

    #[test]
    fn rejects_recency_beyond_one_year() {
        assert!(validate_search(&app(), params(Some(366))).is_err());
        assert!(validate_search(&app(), params(Some(3650))).is_err());
    }

    #[test]
    fn rejects_snippet_outside_range() {
        let mut too_small = params(None);
        too_small.snippet_max_chars = Some(99);
        assert!(validate_search(&app(), too_small).is_err());

        let mut too_large = params(None);
        too_large.snippet_max_chars = Some(2_001);
        assert!(validate_search(&app(), too_large).is_err());
    }

    fn search_response(
        sources: Vec<Source>,
        errors: Vec<(&str, ErrorCode)>,
    ) -> SearchResponse {
        SearchResponse {
            query: "q".into(),
            sources,
            provider_errors: errors
                .into_iter()
                .map(|(provider, code)| {
                    (provider.into(), ProviderError::new(code, "test", 1))
                })
                .collect(),
            meta: SearchMeta {
                elapsed_ms: 0,
                chain_used: Vec::new(),
                budget_exhausted: false,
                result_count: 0,
                omitted_sources: 0,
                truncated: false,
                snippet_only: true,
                cached: false,
                cache_age_secs: 0,
                retry_after_secs: None,
                search_id: "id".into(),
                request_id: "request".into(),
            },
        }
    }

    fn source(url: &str, description: String) -> Source {
        Source {
            url: url.into(),
            provider: "tavily".into(),
            title: "Title".into(),
            description: Some(description),
            published_date: None,
            content: None,
        }
    }

    #[test]
    fn clean_zero_results_is_not_a_tool_error() {
        let response = search_response(
            Vec::new(),
            vec![("chain", ErrorCode::ZeroResults)],
        );
        assert!(!search_is_error(&response));
    }

    #[test]
    fn zero_results_caused_by_a_provider_failure_is_a_tool_error() {
        let response = search_response(
            Vec::new(),
            vec![
                ("tavily", ErrorCode::Timeout),
                ("chain", ErrorCode::ZeroResults),
            ],
        );
        assert!(search_is_error(&response));
    }

    #[test]
    fn sources_with_provider_errors_are_not_a_tool_error() {
        let response = search_response(
            vec![source("https://example.com", "ok".into())],
            vec![("firecrawl", ErrorCode::Http5xx)],
        );
        assert!(!search_is_error(&response));
    }

    #[tokio::test]
    async fn get_sources_uses_the_requested_cap_and_flags_truncation() {
        let app = app();
        let search_id = app.cache.put_with_meta(
            vec![source("https://example.com/a", "x".repeat(900))],
            400,
            "test".into(),
        );
        let server = GatewayServer::new(app);
        let result = server
            .get_sources(Parameters(GetSourcesParams {
                search_id,
                offset: None,
                limit: None,
                snippet_max_chars: Some(150),
            }))
            .await
            .expect("tool call");
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["snippet_max_chars"], 150);
        assert_eq!(structured["truncated"], true);
        let description = structured["sources"][0]["description"]
            .as_str()
            .expect("description");
        assert_eq!(description.chars().count(), 150);
    }

    #[tokio::test]
    async fn get_sources_defaults_to_the_search_cap() {
        let app = app();
        let search_id = app.cache.put_with_meta(
            vec![source("https://example.com/a", "x".repeat(900))],
            300,
            "test".into(),
        );
        let server = GatewayServer::new(app);
        let result = server
            .get_sources(Parameters(GetSourcesParams {
                search_id,
                offset: None,
                limit: None,
                snippet_max_chars: None,
            }))
            .await
            .expect("tool call");
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["snippet_max_chars"], 300);
        assert_eq!(structured["truncated"], true);
    }

    #[test]
    fn map_with_a_successful_provider_and_zero_urls_is_not_an_error() {
        let response = MapResponse {
            base_url: "https://example.com".into(),
            urls: Vec::new(),
            provider: Some("tavily".into()),
            truncated: false,
            omitted_urls: 0,
            provider_errors: BTreeMap::new(),
        };
        assert!(!map_is_error(&response));
        let failed = MapResponse {
            provider: None,
            ..response
        };
        assert!(map_is_error(&failed));
    }

    #[test]
    fn map_budget_removes_urls_and_reports_them() {
        let mut response = MapResponse {
            base_url: "https://example.com".into(),
            urls: (0..50)
                .map(|index| format!("https://example.com/page/{index}"))
                .collect(),
            provider: Some("tavily".into()),
            truncated: false,
            omitted_urls: 0,
            provider_errors: BTreeMap::new(),
        };
        trim_map_budget(&mut response, 500);
        assert!(response.urls.len() < 50);
        assert!(response.truncated);
        assert_eq!(response.omitted_urls, 50 - response.urls.len());
        assert!(map_chars(&response) <= 500);
    }

    #[test]
    fn get_sources_budget_reduces_page_and_advances_offset() {
        let source = |index: usize| crate::model::Source {
            url: format!("https://example.com/{index}"),
            provider: "tavily".into(),
            title: format!("Source {index}"),
            description: Some("x".repeat(800)),
            published_date: None,
            content: None,
        };
        let mut response = GetSourcesResponse {
            search_id: "id".into(),
            total_sources: 10,
            snippet_max_chars: 800,
            sources: (0..10).map(source).collect(),
            next_offset: None,
            truncated: false,
            snippet_only: true,
            error: None,
        };
        trim_get_sources_budget(&mut response, 0, 500);
        assert!(response.sources.len() < 10);
        assert!(response.truncated, "budget trimming must set truncated");
        assert_eq!(
            response.next_offset,
            Some(response.sources.len()),
            "offset must advance past the trimmed page"
        );
        assert!(get_sources_chars(&response) <= 500);
    }
}
