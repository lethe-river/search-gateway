use chrono::{Duration as ChronoDuration, Utc};
use search_gateway::app::App;
use search_gateway::config::{Config, LoadedConfig};
use search_gateway::model::ErrorCode;
use search_gateway::search::SearchParams;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn loaded(config: Config) -> LoadedConfig {
    LoadedConfig {
        config,
        path: None,
        warnings: Vec::new(),
    }
}

fn base_config() -> Config {
    let mut config = Config::default();
    config.tavily.enabled = false;
    config.firecrawl.enabled = false;
    config.searxng.enabled = false;
    config.fetch.allow_private_hosts = true;
    config
}

fn tavily_config(uri: String, keys: Vec<String>) -> Config {
    let mut config = base_config();
    config.tavily.enabled = true;
    config.tavily.base_url = uri;
    config.tavily.keys = keys;
    config
}

fn params(max_results: usize) -> SearchParams {
    SearchParams {
        query: "test query".into(),
        max_results,
        recency_days: None,
        include_domains: Vec::new(),
        exclude_domains: Vec::new(),
        categories: vec!["general".into()],
        snippet_max_chars: 800,
        fresh: false,
    }
}

#[tokio::test]
async fn tavily_search_normalizes_sources() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "title": "Rust",
                    "url": "https://example.com/rust?utm_source=x",
                    "content": "A snippet",
                    "published_date": "2026-01-02T03:04:05+08:00"
                },
                {
                    "title": "",
                    "url": "https://example.com/empty",
                    "content": "no title"
                }
            ]
        })))
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(server.uri(), vec!["k1".into()])))
        .expect("app builds");
    let response = app.search.search(params(10)).await;

    assert_eq!(response.sources.len(), 1);
    assert_eq!(response.sources[0].url, "https://example.com/rust");
    assert_eq!(
        response.sources[0].published_date.as_deref(),
        Some("2026-01-01T19:04:05Z")
    );
    assert_eq!(response.meta.chain_used, vec!["tavily"]);
    assert!(response.provider_errors.is_empty());
}

#[tokio::test]
async fn tavily_rotates_keys_on_auth_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("authorization", "Bearer key1"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("authorization", "Bearer key2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Second key",
                "url": "https://example.com/second",
                "content": "ok"
            }]
        })))
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(
        server.uri(),
        vec!["key1".into(), "key2".into()],
    )))
    .expect("app builds");
    let response = app.search.search(params(10)).await;
    assert_eq!(response.sources.len(), 1);
    assert_eq!(response.sources[0].title, "Second key");

    let doctor = app.doctor.run(false).await;
    let keys = doctor.keyring.get("tavily").expect("tavily keyring");
    assert_eq!(keys.len(), 2);
    assert!(!keys[0].healthy, "first key must be cooling down");
    assert!(keys[1].healthy, "second key must remain usable");
}

#[tokio::test]
async fn firecrawl_tops_up_tavily_results() {
    let tavily = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Tavily one",
                "url": "https://example.com/one",
                "content": "one"
            }]
        })))
        .mount(&tavily)
        .await;

    let firecrawl = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {
                "web": [
                    {"title": "Two", "url": "https://example.com/two", "description": "two"},
                    {"title": "Three", "url": "https://example.com/three", "description": "three"},
                    {"title": "Four", "url": "https://example.com/four", "description": "four"}
                ]
            }
        })))
        .mount(&firecrawl)
        .await;

    let mut config = tavily_config(tavily.uri(), vec!["k1".into()]);
    config.firecrawl.enabled = true;
    config.firecrawl.base_url = firecrawl.uri();
    config.firecrawl.keys = vec!["fc1".into()];

    let app = App::new_lenient(loaded(config)).expect("app builds");
    let response = app.search.search(params(3)).await;

    assert_eq!(response.sources.len(), 3);
    assert_eq!(response.meta.chain_used, vec!["tavily", "firecrawl"]);
    assert!(response.provider_errors.is_empty());
    let providers: Vec<&str> = response
        .sources
        .iter()
        .map(|source| source.provider.as_str())
        .collect();
    assert_eq!(providers, vec!["tavily", "firecrawl", "firecrawl"]);
}

#[tokio::test]
async fn searxng_is_last_resort_after_both_providers_fail() {
    let tavily = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&tavily)
        .await;
    let firecrawl = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&firecrawl)
        .await;
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {"title": "SearX one", "url": "https://example.com/s1", "content": "s1"},
                {"title": "SearX two", "url": "https://example.com/s2", "content": "s2"}
            ],
            "unresponsive_engines": []
        })))
        .mount(&searxng)
        .await;

    let mut config = tavily_config(tavily.uri(), vec!["k1".into()]);
    config.firecrawl.enabled = true;
    config.firecrawl.base_url = firecrawl.uri();
    config.firecrawl.keys = vec!["fc1".into()];
    config.searxng.enabled = true;
    config.searxng.base_url = searxng.uri();

    let app = App::new_lenient(loaded(config)).expect("app builds");
    let response = app.search.search(params(10)).await;

    assert_eq!(response.sources.len(), 2);
    assert_eq!(
        response.meta.chain_used,
        vec!["tavily", "firecrawl", "searxng"]
    );
    assert!(response
        .sources
        .iter()
        .all(|source| source.provider == "searxng"));
    assert!(!response.provider_errors.is_empty());
    assert_eq!(
        response.provider_errors["tavily"].code,
        search_gateway::model::ErrorCode::Http5xx
    );
}

#[tokio::test]
async fn generic_fetch_extracts_article_markdown() {
    let server = MockServer::start().await;
    let html = r#"<html><head><title>Test Article</title></head><body>
        <article>
          <h1>Test Article</h1>
          <p>Hello world. This is the first paragraph of a reasonably long article body that readability should keep.</p>
          <p>This is the second paragraph. It contains enough text to be considered the main content of the page.</p>
          <p>This is the third paragraph, added so the extraction heuristic has enough material to work with.</p>
        </article>
      </body></html>"#;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(html),
        )
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(base_config())).expect("app builds");
    let response = app
        .fetch
        .fetch(&format!("{}/article", server.uri()), None)
        .await;

    assert_eq!(response.extractor, "generic", "{response:?}");
    assert!(response.error.is_none());
    assert!(response.content.contains("Hello world"));
    assert_eq!(response.http_status, Some(200));
    assert!(response.end_of_content, "{response:?}");
    assert_eq!(response.quality, "ok", "{response:?}");
    assert!(response.quality_score >= 45);
}

#[tokio::test]
async fn fetch_continuation_reads_next_chunk_without_network() {
    let server = MockServer::start().await;
    let html = format!(
        "<html><head><title>Long article</title></head><body><article>{}</article></body></html>",
        (0..12)
            .map(|index| format!(
                "<p>Paragraph {index} carries a long stretch of article text so readability keeps it and the extracted Markdown is comfortably longer than one small chunk.</p>"
            ))
            .collect::<String>()
    );
    Mock::given(method("GET"))
        .and(path("/long"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(html),
        )
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(base_config())).expect("app builds");
    let url = format!("{}/long", server.uri());
    let first = app.fetch.fetch_page(&url, Some(200), 0, None).await;
    assert!(first.error.is_none(), "{first:?}");
    assert_eq!(first.content.chars().count(), 200);
    assert!(first.truncated);
    assert!(!first.end_of_content);
    assert_eq!(first.http_status, Some(200));
    let fetch_id = first.fetch_id.clone().expect("fetch_id");
    let next_offset = first.next_offset.expect("next_offset");
    assert_eq!(next_offset, 200);

    let requests_before = server.received_requests().await.expect("requests").len();
    let second = app
        .fetch
        .fetch_page(&url, Some(200), next_offset, Some(&fetch_id))
        .await;
    let requests_after = server.received_requests().await.expect("requests").len();

    assert_eq!(
        requests_before, requests_after,
        "continuation must not hit the network"
    );
    assert!(second.error.is_none(), "{second:?}");
    assert_eq!(second.url, first.url);
    assert!(!second.content.is_empty());
    assert_ne!(second.content, first.content);

    let unknown = app
        .fetch
        .fetch_page(&url, Some(200), 0, Some("does-not-exist"))
        .await;
    let error = unknown.error.expect("unknown fetch id error");
    assert_eq!(error.code, ErrorCode::UnknownFetchId);

    let mismatch = app
        .fetch
        .fetch_page(
            "https://example.com/a-different-page",
            Some(200),
            0,
            Some(&fetch_id),
        )
        .await;
    let error = mismatch.error.expect("fetch id mismatch error");
    assert_eq!(error.code, ErrorCode::UnknownFetchId);
    assert!(error.message.contains("different URL"), "{error:?}");

    let past_end = app
        .fetch
        .fetch_page(&url, Some(200), 100_000, Some(&fetch_id))
        .await;
    assert!(past_end.error.is_none(), "{past_end:?}");
    assert!(past_end.content.is_empty());
    assert!(past_end.end_of_content);
    assert!(!past_end.truncated);
}

#[tokio::test]
async fn boilerplate_page_is_reported_as_low_quality() {
    let server = MockServer::start().await;
    let links = (0..12)
        .map(|index| {
            format!(
                r#"<a href="/page/{index}">首页 登录 注册 分享 扫一扫 点击 返回顶部 版权所有</a><br/>"#
            )
        })
        .collect::<String>();
    let html = format!(
        "<html><head><title>Link farm</title></head><body><article>{links}</article></body></html>"
    );
    Mock::given(method("GET"))
        .and(path("/links"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(html),
        )
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(base_config())).expect("app builds");
    let response = app
        .fetch
        .fetch(&format!("{}/links", server.uri()), None)
        .await;

    assert!(response.error.is_none(), "{response:?}");
    assert_eq!(response.quality, "low", "{response:?}");
    assert!(response.quality_score < 45);
}

#[tokio::test]
async fn local_rate_limit_rejects_excess_search_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "One",
                "url": "https://example.com/one",
                "content": "one"
            }]
        })))
        .mount(&server)
        .await;

    let mut config = tavily_config(server.uri(), vec!["k1".into()]);
    config.rate.max_calls_per_minute = 1;
    let app = App::new_lenient(loaded(config)).expect("app builds");

    let first = app.search.search(params(5)).await;
    assert_eq!(first.sources.len(), 1);

    let mut second_request = params(5);
    second_request.query = "a different query".into();
    let second = app.search.search(second_request).await;

    assert!(second.sources.is_empty());
    assert_eq!(second.meta.chain_used, vec!["rate_limited"]);
    assert_eq!(second.provider_errors["local"].code, ErrorCode::RateLimited);
}


#[tokio::test]
async fn map_falls_back_to_firecrawl() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/map"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/map"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "links": [
                {"url": "https://example.com/a"},
                {"url": "https://example.com/a/"},
                {"url": "https://example.com/b"}
            ]
        })))
        .mount(&server)
        .await;

    let mut config = tavily_config(server.uri(), vec!["k1".into()]);
    config.firecrawl.enabled = true;
    config.firecrawl.base_url = server.uri();
    config.firecrawl.keys = vec!["fc1".into()];

    let app = App::new_lenient(loaded(config)).expect("app builds");
    let response = app.map.map("https://example.com", 10).await;

    assert_eq!(response.provider.as_deref(), Some("firecrawl"));
    assert_eq!(
        response.urls,
        vec!["https://example.com/a", "https://example.com/b"]
    );
    assert_eq!(
        response.provider_errors["tavily"].code,
        search_gateway::model::ErrorCode::Http5xx
    );
}

#[tokio::test]
async fn circuit_breaker_skips_open_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(503).set_body_string("boom"))
        .mount(&server)
        .await;

    let mut config = tavily_config(server.uri(), vec!["k1".into()]);
    config.keyring.transport_retry = 0;
    config.circuit.failure_threshold = 2;
    let app = App::new_lenient(loaded(config)).expect("app builds");

    let first = app.search.search(params(5)).await;
    assert_eq!(first.provider_errors["tavily"].code, ErrorCode::Http5xx);
    let second = app.search.search(params(5)).await;
    assert_eq!(second.provider_errors["tavily"].code, ErrorCode::Http5xx);

    let requests_before = server.received_requests().await.expect("requests").len();
    let third = app.search.search(params(5)).await;
    let requests_after = server.received_requests().await.expect("requests").len();

    assert_eq!(requests_before, requests_after, "open breaker must not hit HTTP");
    assert_eq!(third.provider_errors["tavily"].code, ErrorCode::CircuitOpen);
    assert_eq!(third.provider_errors["tavily"].attempts, 0);
}

#[tokio::test]
async fn repeated_identical_search_uses_query_cache() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Cached source",
                "url": "https://example.com/cached",
                "content": "cached snippet"
            }]
        })))
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(
        server.uri(),
        vec!["k1".into()],
    )))
    .expect("app builds");

    let first = app.search.search(params(5)).await;
    assert_eq!(first.meta.chain_used, vec!["tavily"]);
    let requests_before = server.received_requests().await.expect("requests").len();

    let second = app.search.search(params(5)).await;
    let requests_after = server.received_requests().await.expect("requests").len();

    assert_eq!(requests_before, requests_after, "second search must be cached");
    assert_eq!(second.meta.chain_used, vec!["cache"]);
    assert!(second.meta.cached);
    assert!(second.meta.cache_age_secs < 60, "{:?}", second.meta);
    assert_eq!(second.sources.len(), 1);
    assert_eq!(second.sources[0].url, first.sources[0].url);
    assert!(!first.meta.cached);

    let mut fresh = params(5);
    fresh.fresh = true;
    let third = app.search.search(fresh).await;
    let requests_after_fresh = server.received_requests().await.expect("requests").len();
    assert_eq!(
        requests_after_fresh,
        requests_after + 1,
        "fresh=true must bypass the query cache"
    );
    assert!(!third.meta.cached);
    assert_eq!(third.meta.cache_age_secs, 0);
}

#[tokio::test]
async fn search_trims_snippets_and_response_budget() {
    let server = MockServer::start().await;
    let long = "x".repeat(2_000);
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": (0..5).map(|index| json!({
                "title": format!("Source {index}"),
                "url": format!("https://example.com/{index}"),
                "content": long
            })).collect::<Vec<_>>()
        })))
        .mount(&server)
        .await;

    let mut config = tavily_config(server.uri(), vec!["k1".into()]);
    config.search.snippet_max_chars = 100;
    config.search.response_max_chars = 1_200;
    let app = App::new_lenient(loaded(config)).expect("app builds");

    let mut search = params(5);
    search.snippet_max_chars = 100;
    let response = app.search.search(search.clone()).await;

    assert!(response.meta.truncated, "{response:?}");
    assert!(response.meta.omitted_sources > 0, "{response:?}");
    assert!(response
        .sources
        .iter()
        .all(|source| source.description.as_deref().unwrap_or("").chars().count() <= 100));
    let serialized = serde_json::to_string(&response).expect("serialize");
    assert!(
        serialized.chars().count() <= 1_200,
        "response is {} chars",
        serialized.chars().count()
    );

    let page = app
        .cache
        .get(&response.meta.search_id, 0, 10)
        .expect("full source set");
    assert_eq!(page.total_sources, 5);

    let cached = app.search.search(search).await;
    assert_eq!(cached.meta.chain_used, vec!["cache"]);
    assert!(cached.meta.cached);
    assert!(cached.meta.truncated);
    assert!(serde_json::to_string(&cached).expect("serialize").chars().count() <= 1_200);
}

#[tokio::test]
async fn get_sources_cap_follows_search_and_reuses_source_set() {
    let server = MockServer::start().await;
    let long = "y".repeat(1_500);
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Long",
                "url": "https://example.com/long",
                "content": long
            }]
        })))
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(server.uri(), vec!["k1".into()])))
        .expect("app builds");
    let mut search = params(5);
    search.snippet_max_chars = 2_000;
    let response = app.search.search(search.clone()).await;
    let search_id = response.meta.search_id.clone();

    let page = app.cache.get(&search_id, 0, 10).expect("source set");
    assert_eq!(page.snippet_max_chars, 2_000);
    assert_eq!(
        page.sources[0]
            .description
            .as_deref()
            .unwrap_or_default()
            .chars()
            .count(),
        1_500,
        "the cached description keeps its full length"
    );

    let reused = app.search.search(search).await;
    assert_eq!(reused.meta.search_id, search_id, "cached id must be reused");
}

#[tokio::test]
async fn search_cache_pages_full_result_set() {
    let tavily = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": (0..5).map(|index| json!({
                "title": format!("Source {index}"),
                "url": format!("https://example.com/{index}"),
                "content": "snippet"
            })).collect::<Vec<_>>()
        })))
        .mount(&tavily)
        .await;

    let app = App::new_lenient(loaded(tavily_config(tavily.uri(), vec!["k1".into()])))
        .expect("app builds");
    let response = app.search.search(params(2)).await;
    assert_eq!(response.sources.len(), 2);

    let page = app
        .cache
        .get(&response.meta.search_id, 0, 10)
        .expect("cached search");
    assert_eq!(page.total_sources, 5);
    assert_eq!(page.sources.len(), 5);
    assert!(page.next_offset.is_none());
}

#[tokio::test]
async fn recency_filter_drops_old_dated_sources() {
    let server = MockServer::start().await;
    let recent = (Utc::now() - ChronoDuration::days(2)).to_rfc3339();
    let old = (Utc::now() - ChronoDuration::days(100)).to_rfc3339();
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "title": "Recent",
                    "url": "https://example.com/recent",
                    "content": "recent",
                    "published_date": recent
                },
                {
                    "title": "Old",
                    "url": "https://example.com/old",
                    "content": "old",
                    "published_date": old
                },
                {
                    "title": "Undated",
                    "url": "https://example.com/undated",
                    "content": "undated"
                }
            ]
        })))
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(server.uri(), vec!["k1".into()])))
        .expect("app builds");
    let mut request = params(5);
    request.recency_days = Some(30);
    let response = app.search.search(request).await;

    let titles: Vec<&str> = response
        .sources
        .iter()
        .map(|source| source.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Recent", "Undated"]);
    assert_eq!(response.meta.result_count, 2);
}

#[tokio::test]
async fn origin_4xx_with_firecrawl_content_is_reported_as_warning() {
    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/forbidden"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .mount(&origin)
        .await;

    let firecrawl = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/scrape"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {
                "markdown": "Fallback body that Firecrawl extracted from the 403 page.",
                "metadata": {"title": "Fallback 403 page"}
            }
        })))
        .mount(&firecrawl)
        .await;

    let mut config = base_config();
    config.firecrawl.enabled = true;
    config.firecrawl.base_url = firecrawl.uri();
    config.firecrawl.keys = vec!["fc1".into()];

    let app = App::new_lenient(loaded(config)).expect("app builds");
    let response = app
        .fetch
        .fetch(&format!("{}/forbidden", origin.uri()), None)
        .await;

    assert_eq!(response.http_status, Some(403));
    assert!(response.content.contains("Fallback body"), "{response:?}");
    assert!(response.end_of_content);
    assert!(response
        .fallback_reason
        .as_deref()
        .unwrap_or_default()
        .contains("HTTP 403"));
    assert!(response.error.is_none(), "content must not be a tool error");
    let warning = response.warning.expect("4xx must be reported as a warning");
    assert_eq!(warning.code, ErrorCode::Http4xx);
}

#[tokio::test]
async fn offset_without_fetch_id_is_rejected_before_network() {
    let server = MockServer::start().await;
    let app = App::new_lenient(loaded(base_config())).expect("app builds");
    let response = app
        .fetch
        .fetch_page(&format!("{}/anything", server.uri()), Some(200), 10, None)
        .await;

    let error = response.error.expect("offset without fetch_id");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty(),
        "the guard must run before any request"
    );
}

#[tokio::test]
async fn not_found_skips_the_firecrawl_fallback() {
    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&origin)
        .await;

    let firecrawl = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/scrape"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"markdown": "should not be used"}
        })))
        .mount(&firecrawl)
        .await;

    let mut config = base_config();
    config.firecrawl.enabled = true;
    config.firecrawl.base_url = firecrawl.uri();
    config.firecrawl.keys = vec!["fc1".into()];

    let app = App::new_lenient(loaded(config)).expect("app builds");
    let response = app
        .fetch
        .fetch(&format!("{}/missing", origin.uri()), None)
        .await;

    assert_eq!(response.http_status, Some(404));
    assert!(response.content.is_empty());
    let error = response.error.expect("404 must be reported");
    assert_eq!(error.code, ErrorCode::Http4xx);
    assert!(
        firecrawl
            .received_requests()
            .await
            .expect("requests")
            .is_empty(),
        "a 404 must not spend a Firecrawl call"
    );
}

#[tokio::test]
async fn retry_after_is_reported_when_all_keys_are_cooling() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "42")
                .set_body_string("slow down"),
        )
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(server.uri(), vec!["k1".into()])))
        .expect("app builds");
    let response = app.search.search(params(5)).await;

    assert!(response.sources.is_empty());
    assert_eq!(
        response.provider_errors["tavily"].code,
        ErrorCode::AllKeysCooling
    );
    assert_eq!(
        response.provider_errors["tavily"].retry_after_secs,
        Some(42)
    );
    assert_eq!(response.meta.retry_after_secs, Some(42));
}

#[tokio::test]
async fn searxng_zero_results_include_unresponsive_engine_names() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [],
            "unresponsive_engines": [["bing", "timeout"], ["google cse", "captcha"]]
        })))
        .mount(&server)
        .await;

    let mut config = base_config();
    config.searxng.enabled = true;
    config.searxng.base_url = server.uri();
    let app = App::new_lenient(loaded(config)).expect("app builds");
    let response = app.search.search(params(5)).await;

    assert!(response.sources.is_empty());
    let message = &response.provider_errors["searxng"].message;
    assert!(message.contains("bing (timeout)"), "{message}");
    assert!(message.contains("google cse (captcha)"), "{message}");
    assert!(response.meta.retry_after_secs.is_none());
}

#[tokio::test]
async fn origin_4xx_without_fallback_reports_http_4xx() {
    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&origin)
        .await;

    let app = App::new_lenient(loaded(base_config())).expect("app builds");
    let response = app
        .fetch
        .fetch(&format!("{}/missing", origin.uri()), None)
        .await;

    assert_eq!(response.http_status, Some(404));
    assert!(response.content.is_empty());
    assert!(!response.end_of_content);
    let error = response.error.expect("404 must be reported");
    assert_eq!(error.code, ErrorCode::Http4xx);
}

#[tokio::test]
async fn bad_request_does_not_open_provider_breaker() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .mount(&server)
        .await;

    let mut config = tavily_config(server.uri(), vec!["k1".into()]);
    config.keyring.transport_retry = 0;
    config.circuit.failure_threshold = 1;
    let app = App::new_lenient(loaded(config)).expect("app builds");

    let first = app.search.search(params(5)).await;
    assert_eq!(
        first.provider_errors["tavily"].code,
        ErrorCode::InvalidRequest
    );

    let mut second_request = params(5);
    second_request.query = "another query".into();
    let second = app.search.search(second_request).await;
    assert_eq!(
        second.provider_errors["tavily"].code,
        ErrorCode::InvalidRequest
    );

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 2, "a 400 must not open the breaker");
    let doctor = app.doctor.run(false).await;
    assert_eq!(doctor.breaker["tavily.search"].state, "closed");
}

#[tokio::test]
async fn zero_results_reports_chain_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(server.uri(), vec!["k1".into()])))
        .expect("app builds");
    let response = app.search.search(params(5)).await;

    assert!(response.sources.is_empty());
    assert_eq!(
        response.provider_errors["chain"].code,
        ErrorCode::ZeroResults
    );
}

#[tokio::test]
async fn doctor_without_live_reports_unknown() {
    let server = MockServer::start().await;
    let app = App::new_lenient(loaded(tavily_config(server.uri(), vec!["k1".into()])))
        .expect("app builds");

    let report = app.doctor.run(false).await;
    assert_eq!(
        report.status,
        search_gateway::model::DoctorOverallStatus::Unknown
    );
    assert!(!report.live_verified);
    let tavily = report
        .checks
        .iter()
        .find(|check| check.name == "tavily_probe")
        .expect("tavily probe");
    assert_eq!(tavily.status, search_gateway::model::CheckStatus::Skipped);
    let fetch = report
        .checks
        .iter()
        .find(|check| check.name == "fetch_probe")
        .expect("fetch probe");
    assert_eq!(fetch.status, search_gateway::model::CheckStatus::Skipped);
}

#[tokio::test]
async fn doctor_live_runs_a_free_fetch_probe() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Probe",
                "url": "https://example.com/probe",
                "content": "probe"
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<html><body>ok</body></html>"),
        )
        .mount(&server)
        .await;

    let mut config = tavily_config(server.uri(), vec!["k1".into()]);
    config.doctor.fetch_probe_url = format!("{}/", server.uri());
    let app = App::new_lenient(loaded(config)).expect("app builds");

    let report = app.doctor.run(true).await;
    assert!(report.live_verified);
    assert_eq!(
        report.status,
        search_gateway::model::DoctorOverallStatus::Ok,
        "{report:?}"
    );
    let fetch = report
        .checks
        .iter()
        .find(|check| check.name == "fetch_probe")
        .expect("fetch probe");
    assert_eq!(fetch.status, search_gateway::model::CheckStatus::Ok);
    assert!(fetch.detail.contains("200"), "{}", fetch.detail);
}

#[tokio::test]
async fn map_failures_do_not_open_the_search_breaker() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/map"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Search still works",
                "url": "https://example.com/search",
                "content": "ok"
            }]
        })))
        .mount(&server)
        .await;

    let mut config = tavily_config(server.uri(), vec!["k1".into()]);
    config.keyring.transport_retry = 0;
    config.circuit.failure_threshold = 1;
    let app = App::new_lenient(loaded(config)).expect("app builds");

    let map = app.map.map("https://example.com", 5).await;
    assert!(map.urls.is_empty());
    assert_eq!(map.provider_errors["tavily"].code, ErrorCode::Http5xx);

    let search = app.search.search(params(5)).await;
    assert_eq!(search.sources.len(), 1, "map must not block search");

    let doctor = app.doctor.run(false).await;
    assert_eq!(doctor.breaker["tavily.map"].state, "open");
    assert_eq!(doctor.breaker["tavily.search"].state, "closed");
}

#[tokio::test]
async fn map_zero_results_keeps_the_successful_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/map"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
        .mount(&server)
        .await;

    let app = App::new_lenient(loaded(tavily_config(server.uri(), vec!["k1".into()])))
        .expect("app builds");
    let response = app.map.map("https://example.com", 10).await;

    assert!(response.urls.is_empty());
    assert_eq!(response.provider.as_deref(), Some("tavily"));
    assert_eq!(
        response.provider_errors["tavily"].code,
        ErrorCode::ZeroResults
    );
}

#[tokio::test]
async fn map_without_providers_reports_misconfigured() {
    let app = App::new_lenient(loaded(base_config())).expect("app builds");
    let response = app.map.map("https://example.com", 10).await;

    assert!(response.urls.is_empty());
    assert_eq!(
        response.provider_errors["map"].code,
        ErrorCode::Misconfigured
    );
}

#[tokio::test]
async fn searxng_receives_site_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "title": "Example",
                "url": "https://example.com/result",
                "content": "snippet"
            }],
            "unresponsive_engines": []
        })))
        .mount(&server)
        .await;

    let mut config = base_config();
    config.searxng.enabled = true;
    config.searxng.base_url = server.uri();
    let app = App::new_lenient(loaded(config)).expect("app builds");

    let mut request = params(5);
    request.include_domains = vec!["example.com".into()];
    let response = app.search.search(request).await;
    assert_eq!(response.sources.len(), 1);

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
    let query = requests[0]
        .url
        .query_pairs()
        .find(|(name, _)| name == "q")
        .map(|(_, value)| value.into_owned())
        .expect("q parameter");
    assert_eq!(query, "test query site:example.com");
}
