use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, TimeDelta, Utc};
use url::Url;

use crate::model::Source;

const MAX_DESCRIPTION_CHARS: usize = 2_000;

/// Canonicalizes a URL for deduplication and filtering.
pub fn canonical_url(raw: &str) -> Option<String> {
    let mut url = Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str().is_none() {
        return None;
    }

    url.set_fragment(None);

    if let Some(port) = url.port() {
        if Some(port) == url.port_or_known_default() {
            let _ = url.set_port(None);
        }
    }

    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !is_tracking_param(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }

    let path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        url.set_path(path.trim_end_matches('/'));
    }

    Some(url.to_string())
}

fn is_tracking_param(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_")
        || matches!(
            key.as_str(),
            "fbclid" | "gclid" | "dclid" | "msclkid" | "igshid" | "mc_cid" | "mc_eid"
        )
}

pub fn host_matches(host: &str, domain: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let domain = domain
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if domain.is_empty() {
        return false;
    }
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn domain_allowed(url: &str, include: &[String], exclude: &[String]) -> bool {
    let host = Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string));
    let Some(host) = host else {
        return false;
    };
    if exclude.iter().any(|domain| host_matches(&host, domain)) {
        return false;
    }
    include.is_empty() || include.iter().any(|domain| host_matches(&host, domain))
}

/// Normalizes one provider result. Returns `None` when the item is unusable.
pub fn normalize_source(
    source: Source,
    include_domains: &[String],
    exclude_domains: &[String],
) -> Option<Source> {
    let url = canonical_url(&source.url)?;
    if !domain_allowed(&url, include_domains, exclude_domains) {
        return None;
    }
    let title = source.title.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let description = source
        .description
        .map(|text| truncate_chars(text.trim(), MAX_DESCRIPTION_CHARS).0)
        .filter(|text| !text.is_empty());
    Some(Source {
        url,
        provider: source.provider,
        title,
        description,
        published_date: normalize_date(source.published_date.as_deref()),
        content: source.content,
    })
}

/// Cutoff used to enforce `recency_days` locally. Providers only support
/// coarse day/week/month/year hints, so their filter alone is not precise.
pub fn recency_cutoff(days: Option<u32>) -> Option<DateTime<Utc>> {
    days.map(|days| Utc::now() - TimeDelta::days(i64::from(days)))
}

/// Sources without a parseable date are kept: dropping them would lose most
/// results from providers that omit publication dates.
pub fn source_is_recent(source: &Source, cutoff: Option<DateTime<Utc>>) -> bool {
    let Some(cutoff) = cutoff else {
        return true;
    };
    match source.published_date.as_deref().and_then(parse_date) {
        Some(published) => published >= cutoff,
        None => true,
    }
}

/// Normalizes, filters and deduplicates a provider batch while preserving order.
pub fn normalize_sources(
    sources: Vec<Source>,
    include_domains: &[String],
    exclude_domains: &[String],
    seen: &mut HashSet<String>,
    cutoff: Option<DateTime<Utc>>,
) -> Vec<Source> {
    let mut output = Vec::new();
    for source in sources {
        let Some(source) = normalize_source(source, include_domains, exclude_domains) else {
            continue;
        };
        if !source_is_recent(&source, cutoff) {
            continue;
        }
        if seen.insert(source.url.clone()) {
            output.push(source);
        }
    }
    output
}

pub fn parse_date(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(raw) {
        return Some(datetime.with_timezone(&Utc));
    }
    if let Ok(datetime) = DateTime::parse_from_rfc2822(raw) {
        return Some(datetime.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return date
            .and_hms_opt(0, 0, 0)
            .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }
    if let Ok(datetime) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(datetime, Utc));
    }
    None
}

pub fn normalize_date(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(datetime) = parse_date(raw) {
        return Some(datetime.to_rfc3339_opts(SecondsFormat::Secs, true));
    }
    Some(raw.to_string())
}

/// Truncates by characters (not bytes). Returns (text, truncated, original length).
pub fn truncate_chars(text: &str, max_chars: usize) -> (String, bool, usize) {
    let original = text.chars().count();
    if original <= max_chars {
        return (text.to_string(), false, original);
    }
    (text.chars().take(max_chars).collect(), true, original)
}

pub fn url_host(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_tracking_and_trailing_slash() {
        let url = canonical_url(
            "HTTPS://Example.COM:443/a/b/?utm_source=x&b=2&a=1&fbclid=y#frag",
        )
        .unwrap();
        assert_eq!(url, "https://example.com/a/b?b=2&a=1");
    }

    #[test]
    fn keeps_root_slash() {
        assert_eq!(
            canonical_url("https://example.com/").unwrap(),
            "https://example.com/"
        );
    }

    #[test]
    fn rejects_non_http() {
        assert!(canonical_url("ftp://example.com/file").is_none());
        assert!(canonical_url("not a url").is_none());
    }

    #[test]
    fn domain_matching_is_suffix_based() {
        assert!(host_matches("www.example.com", "example.com"));
        assert!(host_matches("example.com", "example.com"));
        assert!(!host_matches("notexample.com", "example.com"));
    }

    #[test]
    fn truncates_by_characters() {
        let (text, truncated, original) = truncate_chars("你好世界", 2);
        assert_eq!(text, "你好");
        assert!(truncated);
        assert_eq!(original, 4);
    }

    #[test]
    fn normalizes_dates() {
        assert_eq!(
            normalize_date(Some("2026-01-02T03:04:05+08:00")).unwrap(),
            "2026-01-01T19:04:05Z"
        );
        assert_eq!(
            normalize_date(Some("2026-01-02")).unwrap(),
            "2026-01-02T00:00:00Z"
        );
        assert_eq!(
            normalize_date(Some("Thu, 29 Jul 2021 00:00:00 GMT")).unwrap(),
            "2021-07-29T00:00:00Z"
        );
        assert_eq!(normalize_date(Some("unknown")).unwrap(), "unknown");
    }

    #[test]
    fn recency_filter_drops_old_dated_sources_and_keeps_undated_ones() {
        let cutoff = recency_cutoff(Some(30)).expect("cutoff");
        let source = |url: &str, date: Option<&str>| Source {
            url: url.into(),
            provider: "tavily".into(),
            title: "Title".into(),
            description: None,
            published_date: date.map(ToString::to_string),
            content: None,
        };

        assert!(source_is_recent(
            &source("https://example.com/recent", Some("2099-01-01")),
            Some(cutoff)
        ));
        assert!(!source_is_recent(
            &source("https://example.com/old", Some("2020-01-01")),
            Some(cutoff)
        ));
        assert!(source_is_recent(
            &source("https://example.com/undated", None),
            Some(cutoff)
        ));
    }

    #[test]
    fn recency_filter_runs_before_deduplication() {
        let cutoff = recency_cutoff(Some(30)).expect("cutoff");
        let mut seen = HashSet::new();
        let sources = vec![
            Source {
                url: "https://example.com/page".into(),
                provider: "tavily".into(),
                title: "Old".into(),
                description: None,
                published_date: Some("2020-01-01".into()),
                content: None,
            },
            Source {
                url: "https://example.com/page".into(),
                provider: "firecrawl".into(),
                title: "Fresh".into(),
                description: None,
                published_date: Some("2099-01-01".into()),
                content: None,
            },
        ];

        let normalized = normalize_sources(
            sources,
            &[],
            &[],
            &mut seen,
            Some(cutoff),
        );
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].title, "Fresh");
    }
}
