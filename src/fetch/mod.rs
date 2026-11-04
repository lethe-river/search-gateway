use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use regex::Regex;
use url::Url;

use crate::budget::Budget;
use crate::config::Config;
use crate::model::{ErrorCode, FetchResponse, ProviderError, ToolError};
use crate::normalize::canonical_url;
use crate::providers::FirecrawlClient;
use crate::rate::RateLimiter;

pub mod generic;
pub mod specialists;

const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const FETCH_CACHE_CONTENT_CHARS: usize = 1_000_000;
const QUALITY_THRESHOLD: u8 = 45;

pub struct FetchEngine {
    cfg: Arc<Config>,
    http: reqwest::Client,
    firecrawl: Option<Arc<FirecrawlClient>>,
    limiter: Arc<RateLimiter>,
    host_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pages: Mutex<VecDeque<FetchPage>>,
}

#[derive(Clone)]
struct FetchPage {
    id: String,
    created: Instant,
    url: String,
    title: Option<String>,
    content: String,
    original_length: usize,
    extractor: String,
    score: u8,
    http_status: Option<u16>,
    retry_after_secs: Option<u64>,
}

enum Document {
    Html(String),
    Text(String),
    Pdf,
    Unsupported(String),
}

struct Fetched {
    status: u16,
    document: Document,
}

struct FetchFailure {
    error: ProviderError,
    status: Option<u16>,
}

struct Candidate {
    extract: generic::Extract,
    extractor: &'static str,
    score: u8,
}

impl FetchEngine {
    pub fn new(
        cfg: Arc<Config>,
        http: reqwest::Client,
        firecrawl: Option<Arc<FirecrawlClient>>,
        limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            cfg,
            http,
            firecrawl,
            limiter,
            host_locks: Mutex::new(HashMap::new()),
            pages: Mutex::new(VecDeque::new()),
        }
    }

    pub async fn fetch(&self, raw_url: &str, max_chars: Option<usize>) -> FetchResponse {
        self.fetch_page(raw_url, max_chars, 0, None).await
    }

    pub async fn fetch_page(
        &self,
        raw_url: &str,
        max_chars: Option<usize>,
        offset: usize,
        fetch_id: Option<&str>,
    ) -> FetchResponse {
        let max_chars = max_chars
            .unwrap_or(self.cfg.fetch.max_chars_default)
            .clamp(1, self.cfg.fetch.max_chars_limit);

        if offset > 0 && fetch_id.is_none() {
            return blocked_response(
                raw_url,
                ErrorCode::InvalidRequest,
                "offset > 0 requires fetch_id; fetch the URL without offset first".into(),
            );
        }

        if let Some(id) = fetch_id {
            return self.continue_page(id, raw_url, offset, max_chars);
        }

        let url = match Url::parse(raw_url) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => url,
            _ => {
                return blocked_response(
                    raw_url,
                    ErrorCode::UnsupportedContentType,
                    "URL must be an absolute http(s) URL".into(),
                );
            }
        };

        if let Err(message) = crate::security::ensure_public_url(
            &url,
            self.cfg.fetch.allow_private_hosts,
        )
        .await
        {
            return blocked_response(&url.to_string(), ErrorCode::Blocked, message);
        }

        let host = url.host_str().unwrap_or_default().to_string();
        let lock = self.host_lock(&host);
        let _guard = lock.lock().await;

        if let Err(retry_after) = self.limiter.acquire() {
            let mut response = blocked_response(
                &url.to_string(),
                ErrorCode::RateLimited,
                format!("local tool rate limit exceeded; retry in {retry_after}s"),
            );
            if let Some(error) = response.error.as_mut() {
                error.retry_after_secs = Some(retry_after);
            }
            return response;
        }

        let deadline = Budget::new(Duration::from_millis(self.cfg.fetch.budget_ms));
        let mut reasons: Vec<String> = Vec::new();
        let mut last_error: Option<ToolError> = None;
        let mut last_retry_after: Option<u64> = None;
        let mut document: Option<Document> = None;
        let mut http_status: Option<u16> = None;

        if let Some(kind) = specialists::classify(&url) {
            let slice = deadline.slice(Duration::from_millis(
                self.cfg.fetch.specialist_timeout_ms,
            ));
            if slice.is_zero() {
                reasons.push("budget exhausted before specialist".into());
            } else {
                match self
                    .fetch_document(&url, &deadline, self.cfg.fetch.specialist_timeout_ms)
                    .await
                {
                    Ok(fetched) => {
                        http_status = Some(fetched.status);
                        document = Some(fetched.document);
                    }
                    Err(failure) => {
                        if let Some(status) = failure.status {
                            http_status = Some(status);
                        }
                        if failure.error.code == ErrorCode::Blocked {
                            return blocked_response(
                                &url.to_string(),
                                ErrorCode::Blocked,
                                failure.error.message,
                            );
                        }
                        last_retry_after =
                            failure.error.retry_after_secs.or(last_retry_after);
                        last_error = Some(ToolError::from_provider(&failure.error));
                        reasons.push(format!("{}: {}", kind.extractor(), failure.error.message));
                    }
                }
            }

            if let Some(Document::Html(html)) = &document {
                if let Some(mut extract) = specialists::extract(kind, html, &url) {
                    if extract.title.is_none() {
                        extract.title = generic::html_title(html);
                    }
                    let score = score_content(&extract.markdown, extract.title.as_deref(), true);
                    return self.finish(
                        &url,
                        extract,
                        kind.extractor(),
                        score,
                        reasons,
                        max_chars,
                        offset,
                        http_status,
                        last_retry_after,
                    );
                }
                reasons.push(format!("{}: no structured content", kind.extractor()));
            } else if let Some(Document::Unsupported(content_type)) = &document {
                return unsupported(&url, content_type.clone(), reasons, http_status);
            }
        }

        let mut candidate: Option<Candidate> = None;
        match document.take() {
            Some(Document::Html(html)) => {
                candidate = generic_candidate(&html, &url, &mut reasons);
            }
            Some(Document::Text(text)) => {
                candidate = text_candidate(text, &mut reasons);
            }
            Some(Document::Pdf) => {
                reasons.push("generic: PDF delegated to Firecrawl".into());
                last_error = Some(ToolError::new(
                    ErrorCode::UnsupportedContentType,
                    "PDF extraction requires Firecrawl",
                ));
            }
            Some(Document::Unsupported(content_type)) => {
                return unsupported(&url, content_type, reasons, http_status);
            }
            None => {
                let slice = deadline.slice(Duration::from_millis(
                    self.cfg.fetch.generic_timeout_ms,
                ));
                if slice.is_zero() {
                    reasons.push("budget exhausted before generic fetch".into());
                } else {
                    match self
                        .fetch_document(&url, &deadline, self.cfg.fetch.generic_timeout_ms)
                        .await
                    {
                        Ok(fetched) => {
                            http_status = Some(fetched.status);
                            match fetched.document {
                                Document::Html(html) => {
                                    candidate = generic_candidate(&html, &url, &mut reasons);
                                }
                                Document::Text(text) => {
                                    candidate = text_candidate(text, &mut reasons);
                                }
                                Document::Pdf => {
                                    reasons.push("generic: PDF delegated to Firecrawl".into());
                                    last_error = Some(ToolError::new(
                                        ErrorCode::UnsupportedContentType,
                                        "PDF extraction requires Firecrawl",
                                    ));
                                }
                                Document::Unsupported(content_type) => {
                                    return unsupported(
                                        &url,
                                        content_type,
                                        reasons,
                                        http_status,
                                    );
                                }
                            }
                        }
                        Err(failure) => {
                            if let Some(status) = failure.status {
                                http_status = Some(status);
                            }
                            if failure.error.code == ErrorCode::Blocked {
                                return blocked_response(
                                    &url.to_string(),
                                    ErrorCode::Blocked,
                                    failure.error.message,
                                );
                            }
                            last_retry_after =
                                failure.error.retry_after_secs.or(last_retry_after);
                            last_error = Some(ToolError::from_provider(&failure.error));
                            reasons.push(format!("generic: {}", failure.error.message));
                        }
                    }
                }
            }
        }

        if let Some(candidate) = &candidate {
            if candidate.score >= QUALITY_THRESHOLD {
                return self.finish(
                    &url,
                    candidate.extract.clone(),
                    candidate.extractor,
                    candidate.score,
                    reasons,
                    max_chars,
                    offset,
                    http_status,
                    last_retry_after,
                );
            }
            reasons.push(format!(
                "{}: low extraction quality ({})",
                candidate.extractor, candidate.score
            ));
        }

        let mut best = candidate;
        // A 4xx other than 403/429 means the origin refused this URL; scraping
        // the same URL through Firecrawl wastes a paid call.
        let firecrawl_allowed = http_status
            .map(|status| {
                !(400..500).contains(&status) || matches!(status, 403 | 429)
            })
            .unwrap_or(true);
        if !firecrawl_allowed {
            reasons.push(format!(
                "firecrawl: skipped after HTTP {}",
                http_status.unwrap_or_default()
            ));
        } else if let Some(client) = &self.firecrawl {
            let slice = deadline.slice(Duration::from_millis(
                self.cfg.fetch.firecrawl_timeout_ms,
            ));
            if slice.is_zero() {
                reasons.push("budget exhausted before Firecrawl".into());
            } else {
                match client.scrape(url.as_str(), slice).await {
                    Ok(page) => {
                        let extract = generic::Extract {
                            title: page.title,
                            markdown: page.markdown,
                        };
                        let score =
                            score_content(&extract.markdown, extract.title.as_deref(), false);
                        let firecrawl = Candidate {
                            extract,
                            extractor: "firecrawl",
                            score,
                        };
                        let use_firecrawl = best
                            .as_ref()
                            .map(|current| firecrawl.score > current.score)
                            .unwrap_or(true);
                        if use_firecrawl {
                            best = Some(firecrawl);
                        }
                    }
                    Err(error) => {
                        last_retry_after = error.retry_after_secs.or(last_retry_after);
                        last_error = Some(ToolError::from_provider(&error));
                        reasons.push(format!("firecrawl: {}", error.message));
                    }
                }
            }
        } else {
            reasons.push("firecrawl: not configured".into());
        }

        if let Some(best) = best {
            return self.finish(
                &url,
                best.extract,
                best.extractor,
                best.score,
                reasons,
                max_chars,
                offset,
                http_status,
                last_retry_after,
            );
        }

        let mut error = http_4xx_error(http_status)
            .or(last_error)
            .unwrap_or_else(|| {
                ToolError::new(ErrorCode::Unreachable, "all extraction levels failed")
            });
        if error.retry_after_secs.is_none() {
            error.retry_after_secs = last_retry_after;
        }
        FetchResponse {
            url: url.to_string(),
            title: None,
            content: String::new(),
            original_length: 0,
            truncated: false,
            end_of_content: false,
            http_status,
            extractor: "none".into(),
            quality: "unknown".into(),
            quality_score: 0,
            citation_ready: false,
            citation_caveat: None,
            fetch_id: None,
            next_offset: None,
            fallback_reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
            warning: None,
            error: Some(error),
        }
    }

    fn finish(
        &self,
        url: &Url,
        extract: generic::Extract,
        extractor: &str,
        score: u8,
        reasons: Vec<String>,
        max_chars: usize,
        offset: usize,
        http_status: Option<u16>,
        retry_after_secs: Option<u64>,
    ) -> FetchResponse {
        let full_length = extract.markdown.chars().count();
        let stored: String = if full_length > FETCH_CACHE_CONTENT_CHARS {
            extract
                .markdown
                .chars()
                .take(FETCH_CACHE_CONTENT_CHARS)
                .collect()
        } else {
            extract.markdown
        };
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut pages = self.pages.lock().unwrap_or_else(|error| error.into_inner());
            let ttl = Duration::from_secs(self.cfg.cache.page_ttl_secs);
            let now = Instant::now();
            pages.retain(|page| now.duration_since(page.created) < ttl);
            pages.push_front(FetchPage {
                id: id.clone(),
                created: now,
                url: url.to_string(),
                title: extract.title.clone(),
                content: stored.clone(),
                original_length: full_length,
                extractor: extractor.to_string(),
                score,
                http_status,
                retry_after_secs,
            });
            while pages.len() > self.cfg.cache.max_pages {
                pages.pop_back();
            }
        }

        let (content, next_offset) = slice_page(&stored, offset, max_chars);
        FetchResponse {
            url: url.to_string(),
            title: extract.title,
            content,
            original_length: full_length,
            truncated: next_offset.is_some(),
            end_of_content: next_offset.is_none(),
            http_status,
            extractor: extractor.to_string(),
            quality: quality_label(score).into(),
            quality_score: score,
            citation_ready: false,
            citation_caveat: None,
            fetch_id: Some(id),
            next_offset,
            fallback_reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
            warning: http_4xx_warning(http_status, retry_after_secs),
            error: None,
        }
    }

    fn continue_page(
        &self,
        id: &str,
        raw_url: &str,
        offset: usize,
        max_chars: usize,
    ) -> FetchResponse {
        let page = {
            let mut pages = self.pages.lock().unwrap_or_else(|error| error.into_inner());
            let ttl = Duration::from_secs(self.cfg.cache.page_ttl_secs);
            let now = Instant::now();
            pages.retain(|page| now.duration_since(page.created) < ttl);
            pages.iter().find(|page| page.id == id).cloned()
        };
        let Some(page) = page else {
            return blocked_response(
                raw_url,
                ErrorCode::UnknownFetchId,
                "fetch_id is unknown or has expired".into(),
            );
        };
        let same_page = match (canonical_url(raw_url), canonical_url(&page.url)) {
            (Some(requested), Some(cached)) => requested == cached,
            _ => raw_url == page.url,
        };
        if !same_page {
            return blocked_response(
                raw_url,
                ErrorCode::UnknownFetchId,
                "fetch_id belongs to a different URL; pass the URL that produced it".into(),
            );
        }

        let (content, next_offset) = slice_page(&page.content, offset, max_chars);
        FetchResponse {
            url: page.url,
            title: page.title,
            content,
            original_length: page.original_length,
            truncated: next_offset.is_some(),
            end_of_content: next_offset.is_none(),
            http_status: page.http_status,
            extractor: page.extractor,
            quality: quality_label(page.score).into(),
            quality_score: page.score,
            citation_ready: false,
            citation_caveat: None,
            fetch_id: Some(page.id),
            next_offset,
            fallback_reason: None,
            warning: http_4xx_warning(page.http_status, page.retry_after_secs),
            error: None,
        }
    }

    async fn fetch_document(
        &self,
        url: &Url,
        deadline: &Budget,
        stage_timeout_ms: u64,
    ) -> Result<Fetched, FetchFailure> {
        let mut current = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            if let Err(message) =
                crate::security::ensure_public_url(&current, self.cfg.fetch.allow_private_hosts)
                    .await
            {
                return Err(FetchFailure {
                    error: ProviderError::new(ErrorCode::Blocked, message, 1),
                    status: None,
                });
            }

            let slice = deadline.slice(Duration::from_millis(stage_timeout_ms));
            if slice.is_zero() {
                return Err(FetchFailure {
                    error: ProviderError::new(
                        ErrorCode::BudgetExhausted,
                        "fetch budget exhausted",
                        1,
                    ),
                    status: None,
                });
            }
            let response = match self.http.get(current.clone()).timeout(slice).send().await {
                Ok(response) => response,
                Err(error) => {
                    return Err(FetchFailure {
                        error: ProviderError::new(
                            if error.is_timeout() {
                                ErrorCode::Timeout
                            } else {
                                ErrorCode::Unreachable
                            },
                            error.to_string(),
                            1,
                        ),
                        status: None,
                    });
                }
            };

            let status = response.status();
            let retry_after_secs =
                crate::providers::http::retry_after_secs(response.headers());
            if status.is_redirection() {
                let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                else {
                    return Err(FetchFailure {
                        error: ProviderError::new(
                            ErrorCode::InvalidResponse,
                            "redirect response has no Location header",
                            1,
                        ),
                        status: Some(status.as_u16()),
                    });
                };
                let next = match redirect_target(&current, location) {
                    Ok(next) => next,
                    Err(error) => {
                        return Err(FetchFailure {
                            error,
                            status: Some(status.as_u16()),
                        });
                    }
                };
                current = next;
                continue;
            }

            if !status.is_success() {
                let code = if status.as_u16() == 429 || status.is_server_error() {
                    ErrorCode::Unreachable
                } else if status.is_client_error() {
                    ErrorCode::Http4xx
                } else {
                    ErrorCode::InvalidResponse
                };
                return Err(FetchFailure {
                    error: ProviderError::new(code, format!("HTTP {}", status.as_u16()), 1)
                        .with_retry_after(retry_after_secs),
                    status: Some(status.as_u16()),
                });
            }
            let status_code = status.as_u16();

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase();

            if content_type.contains("application/pdf") {
                return Ok(Fetched {
                    status: status_code,
                    document: Document::Pdf,
                });
            }
            if content_type.contains("text/html")
                || content_type.contains("application/xhtml")
            {
                let bytes = match read_body_limited(response, MAX_BODY_BYTES).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return Err(FetchFailure {
                            error,
                            status: Some(status_code),
                        });
                    }
                };
                return Ok(Fetched {
                    status: status_code,
                    document: Document::Html(decode_text(&bytes, &content_type)),
                });
            }
            if content_type.starts_with("text/")
                || content_type.contains("json")
                || content_type.contains("xml")
            {
                let bytes = match read_body_limited(response, MAX_BODY_BYTES).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return Err(FetchFailure {
                            error,
                            status: Some(status_code),
                        });
                    }
                };
                return Ok(Fetched {
                    status: status_code,
                    document: Document::Text(decode_text(&bytes, &content_type)),
                });
            }

            let bytes = match read_body_limited(response, MAX_BODY_BYTES).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(FetchFailure {
                        error,
                        status: Some(status_code),
                    });
                }
            };
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let lower = text
                    .chars()
                    .take(256)
                    .collect::<String>()
                    .to_ascii_lowercase();
                if lower.contains("<html") || lower.contains("<!doctype html") {
                    return Ok(Fetched {
                        status: status_code,
                        document: Document::Html(text.to_string()),
                    });
                }
                return Ok(Fetched {
                    status: status_code,
                    document: Document::Text(text.to_string()),
                });
            }
            return Ok(Fetched {
                status: status_code,
                document: Document::Unsupported(if content_type.is_empty() {
                    "unknown".into()
                } else {
                    content_type
                }),
            });
        }

        Err(FetchFailure {
            error: ProviderError::new(ErrorCode::InvalidResponse, "too many redirects", 1),
            status: None,
        })
    }

    fn host_lock(&self, host: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.host_locks.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(host.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

fn generic_candidate(
    html: &str,
    url: &Url,
    reasons: &mut Vec<String>,
) -> Option<Candidate> {
    let Some(mut extract) = generic::extract(html, url.as_str()) else {
        reasons.push("generic: readability produced no content".into());
        return None;
    };
    if extract.title.is_none() {
        extract.title = generic::html_title(html);
    }
    let score = score_content(&extract.markdown, extract.title.as_deref(), false);
    Some(Candidate {
        extract,
        extractor: "generic",
        score,
    })
}

fn text_candidate(text: String, reasons: &mut Vec<String>) -> Option<Candidate> {
    if text.trim().is_empty() {
        reasons.push("generic: empty text body".into());
        return None;
    }
    let extract = generic::Extract {
        title: None,
        markdown: text,
    };
    let score = score_content(&extract.markdown, None, false);
    Some(Candidate {
        extract,
        extractor: "generic",
        score,
    })
}

fn redirect_target(current: &Url, location: &str) -> Result<Url, ProviderError> {
    let next = current.join(location).map_err(|error| {
        ProviderError::new(
            ErrorCode::InvalidResponse,
            format!("invalid redirect target: {error}"),
            1,
        )
    })?;
    if !matches!(next.scheme(), "http" | "https") {
        return Err(ProviderError::new(
            ErrorCode::Blocked,
            "redirected to a non-http(s) URL",
            1,
        ));
    }
    Ok(next)
}

fn slice_page(content: &str, offset: usize, max_chars: usize) -> (String, Option<usize>) {    let total = content.chars().count();
    if offset >= total {
        return (String::new(), None);
    }
    let slice: String = content.chars().skip(offset).take(max_chars).collect();
    let end = offset.saturating_add(slice.chars().count());
    (slice, (end < total).then_some(end))
}

fn quality_label(score: u8) -> &'static str {
    if score >= QUALITY_THRESHOLD {
        "ok"
    } else {
        "low"
    }
}

/// CJK characters carry roughly two columns of information per code point.
/// Weighting them keeps Chinese, Japanese and Korean prose from being scored
/// as thin content purely because the text is short in character count.
fn char_weight(character: char) -> usize {
    if matches!(
        character,
        '\u{3040}'..='\u{30ff}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{ac00}'..='\u{d7af}'
            | '\u{f900}'..='\u{faff}'
    ) {
        2
    } else {
        1
    }
}

fn weighted_len(text: &str) -> usize {
    text.chars().map(char_weight).sum()
}

fn score_content(markdown: &str, title: Option<&str>, specialist: bool) -> u8 {
    let raw_chars = markdown.chars().count();
    if raw_chars == 0 {
        return 0;
    }
    let text_chars: usize = markdown
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(char_weight)
        .sum();
    let link_chars: usize = link_regex()
        .find_iter(markdown)
        .map(|matched| matched.as_str().chars().count())
        .sum();
    let link_ratio = link_chars as f64 / raw_chars as f64;
    let longest_line = markdown
        .lines()
        .map(weighted_len)
        .max()
        .unwrap_or(0);
    let prose_lines = markdown
        .lines()
        .filter(|line| weighted_len(line) >= 40 && !line.trim_start().starts_with('|'))
        .count();
    let lower = markdown.to_ascii_lowercase();
    let boilerplate = BOILERPLATE_MARKERS
        .iter()
        .map(|marker| lower.matches(marker).count())
        .sum::<usize>();
    // A fallback that leaves raw HTML in the "markdown" is not article text.
    let html_tags = markdown.matches('<').count();

    let mut score: i32 = 100;
    if text_chars < 300 {
        score -= 40;
    } else if text_chars < 800 {
        score -= 20;
    } else if text_chars < 1_500 {
        score -= 5;
    }
    if link_ratio > 0.50 {
        score -= 45;
    } else if link_ratio > 0.35 {
        score -= 30;
    } else if link_ratio > 0.25 {
        score -= 15;
    } else if link_ratio > 0.15 {
        score -= 5;
    }
    if longest_line < 80 {
        score -= 20;
    } else if longest_line < 150 {
        score -= 10;
    }
    if prose_lines == 0 {
        score -= 20;
    } else if prose_lines < 3 {
        score -= 10;
    }
    if boilerplate >= 6 {
        score -= 30;
    } else if boilerplate >= 3 {
        score -= 15;
    } else if boilerplate >= 1 {
        score -= 5;
    }
    if html_tags >= 20 {
        score -= 30;
    } else if html_tags >= 5 {
        score -= 10;
    }
    if title.is_none() {
        score -= 5;
    }
    // Reference documentation is link-heavy by nature; code blocks and
    // headings are substantive content even when prose lines are short.
    let code_fences = markdown.matches("```").count() / 2;
    let headings = markdown
        .lines()
        .filter(|line| line.starts_with('#'))
        .count();
    if code_fences >= 2 {
        score += 15;
    } else if code_fences == 1 {
        score += 5;
    }
    if headings >= 5 {
        score += 10;
    } else if headings >= 2 {
        score += 5;
    }
    if specialist {
        score += 15;
    }
    score.clamp(0, 100) as u8
}

fn link_regex() -> &'static Regex {
    static LINK_RE: OnceLock<Regex> = OnceLock::new();
    LINK_RE.get_or_init(|| {
        Regex::new(r"\[[^\]]*\]\([^)]*\)").expect("link regex is valid")
    })
}

const BOILERPLATE_MARKERS: &[&str] = &[
    "首页",
    "登录",
    "注册",
    "版权所有",
    "返回顶部",
    "点击",
    "分享",
    "扫一扫",
    "copyright",
    "all rights reserved",
    "privacy policy",
    "terms of service",
    "cookie",
    "subscribe",
    "sign in",
    "sign up",
    "read more",
    "navigation",
];

async fn read_body_limited(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, ProviderError> {
    if let Some(length) = response.content_length() {
        if length > max as u64 {
            return Err(ProviderError::new(
                ErrorCode::InvalidResponse,
                format!("response body is too large ({length} bytes)"),
                1,
            ));
        }
    }
    let mut body = Vec::new();
    loop {
        let chunk = response.chunk().await.map_err(|error| {
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
        match chunk {
            Some(chunk) => {
                if body.len().saturating_add(chunk.len()) > max {
                    return Err(ProviderError::new(
                        ErrorCode::InvalidResponse,
                        format!("response body exceeds the {max}-byte limit"),
                        1,
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            None => break,
        }
    }
    Ok(body)
}

fn decode_text(bytes: &[u8], content_type: &str) -> String {
    let charset = header_charset(content_type).or_else(|| meta_charset(bytes));
    if let Some(label) = charset {
        if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
            let (text, _, _) = encoding.decode(bytes);
            return text.into_owned();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn header_charset(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if name.trim().eq_ignore_ascii_case("charset") {
            Some(value.trim().trim_matches('"').to_string())
        } else {
            None
        }
    })
}

/// Many pages served without a charset header declare one in the first few
/// kilobytes of HTML. Only the ASCII tag area is inspected.
fn meta_charset(bytes: &[u8]) -> Option<String> {
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(8 * 1024)]);
    meta_charset_regex()
        .captures(&prefix)?
        .get(1)
        .map(|matched| matched.as_str().to_string())
}

fn meta_charset_regex() -> &'static Regex {
    static META_CHARSET_RE: OnceLock<Regex> = OnceLock::new();
    META_CHARSET_RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+charset\s*=\s*["']?\s*([a-z0-9_\-]+)"#)
            .expect("meta charset regex is valid")
    })
}

fn http_4xx_error(status: Option<u16>) -> Option<ToolError> {
    status
        .filter(|status| (400..500).contains(status))
        .map(|status| ToolError::new(ErrorCode::Http4xx, format!("origin returned HTTP {status}")))
}

fn http_4xx_warning(
    status: Option<u16>,
    retry_after_secs: Option<u64>,
) -> Option<ToolError> {
    http_4xx_error(status).map(|mut warning| {
        warning.message = format!(
            "origin returned HTTP {}; content may be an error page or come from a fallback extractor",
            status.unwrap_or_default()
        );
        warning.retry_after_secs = retry_after_secs;
        warning
    })
}

fn unsupported(
    url: &Url,
    content_type: String,
    reasons: Vec<String>,
    http_status: Option<u16>,
) -> FetchResponse {
    blocked_response(
        &url.to_string(),
        ErrorCode::UnsupportedContentType,
        format!("unsupported content type: {content_type}"),
    )
    .with_http_status(http_status)
    .with_fallback_reason(reasons)
}

fn blocked_response(raw_url: &str, code: ErrorCode, message: String) -> FetchResponse {
    FetchResponse {
        url: raw_url.to_string(),
        title: None,
        content: String::new(),
        original_length: 0,
        truncated: false,
        end_of_content: false,
        http_status: None,
        extractor: "none".into(),
        quality: "unknown".into(),
        quality_score: 0,
        citation_ready: false,
        citation_caveat: None,
        fetch_id: None,
        next_offset: None,
        fallback_reason: None,
        warning: None,
        error: Some(ToolError::new(code, message)),
    }
}

impl FetchResponse {
    fn with_http_status(mut self, status: Option<u16>) -> Self {
        self.http_status = status;
        self
    }

    fn with_fallback_reason(mut self, reasons: Vec<String>) -> Self {
        self.fallback_reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_target_rejects_non_http_schemes() {
        let current = Url::parse("https://example.com/a").expect("url");
        let error = redirect_target(&current, "file:///etc/passwd").expect_err("scheme block");
        assert_eq!(error.code, ErrorCode::Blocked);
    }

    #[test]
    fn redirect_target_resolves_relative_locations() {
        let current = Url::parse("https://example.com/a/b").expect("url");
        let next = redirect_target(&current, "../c").expect("relative redirect");
        assert_eq!(next.as_str(), "https://example.com/c");
    }

    #[test]
    fn reference_documentation_scores_as_usable_content() {
        let mut markdown = String::new();
        for index in 0..6 {
            markdown.push_str(&format!("## Section {index}\n\n"));
            markdown.push_str(
                "A paragraph of documentation prose long enough to look like real content.\n\n",
            );
            markdown.push_str("```rust\nlet value = compute();\n```\n\n");
            markdown.push_str(
                "- [Item](https://example.com/item)\n- [Other](https://example.com/other)\n\n",
            );
        }
        let score = score_content(&markdown, Some("Reference"), false);
        assert!(score >= QUALITY_THRESHOLD, "score was {score}");
    }

    #[test]
    fn short_cjk_prose_scores_as_usable_content() {
        let paragraph = "这是一段用于测试正文质量评分的中文内容，长度并不算很长。";
        let markdown = (0..20)
            .map(|index| format!("## 第{index}节\n\n{paragraph}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let score = score_content(&markdown, Some("中文文章"), false);
        assert!(score >= QUALITY_THRESHOLD, "score was {score}");
    }

    #[test]
    fn meta_charset_is_used_when_the_header_omits_it() {
        let (encoded, _, _) = encoding_rs::GBK.encode("中文测试内容");
        let mut page = b"<html><head><meta charset=\"gbk\"></head><body>".to_vec();
        page.extend_from_slice(&encoded);
        page.extend_from_slice(b"</body></html>");
        let text = decode_text(&page, "text/html");
        assert!(text.contains("中文测试内容"), "decoded as {text}");
    }

    #[test]
    fn link_farm_scores_below_threshold() {
        let markdown = (0..20)
            .map(|index| {
                format!("[首页 登录 注册 分享 扫一扫](https://example.com/{index})")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let score = score_content(&markdown, Some("Links"), false);
        assert!(score < QUALITY_THRESHOLD, "score was {score}");
    }
}
