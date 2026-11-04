use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};

use super::generic::Extract;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Specialist {
    GitHub,
    StackExchange,
    Arxiv,
    Wikipedia,
}

impl Specialist {
    pub fn extractor(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::StackExchange => "stackexchange",
            Self::Arxiv => "arxiv",
            Self::Wikipedia => "wikipedia",
        }
    }
}

/// Selects a specialist only for the structured pages it can actually parse.
pub fn classify(url: &url::Url) -> Option<Specialist> {
    let host = url.host_str()?.to_ascii_lowercase();
    let path = url.path();

    if matches!(host.as_str(), "github.com" | "www.github.com") {
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        if segments.len() >= 4 {
            if matches!(segments[2], "issues" | "pull" | "discussions")
                && segments[3].chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                return Some(Specialist::GitHub);
            }
            if segments[0] == "orgs" && segments[2] == "discussions" {
                return Some(Specialist::GitHub);
            }
        }
    }

    let stackexchange_host = host == "stackoverflow.com"
        || host.ends_with(".stackexchange.com")
        || matches!(
            host.as_str(),
            "superuser.com" | "serverfault.com" | "askubuntu.com" | "mathoverflow.net"
        );
    if stackexchange_host && path.starts_with("/questions/") {
        return Some(Specialist::StackExchange);
    }

    if matches!(host.as_str(), "arxiv.org" | "www.arxiv.org") && path.starts_with("/abs/") {
        return Some(Specialist::Arxiv);
    }

    if host.ends_with(".wikipedia.org") && path.starts_with("/wiki/") {
        return Some(Specialist::Wikipedia);
    }

    None
}

pub fn extract(kind: Specialist, html: &str, url: &url::Url) -> Option<Extract> {
    match kind {
        Specialist::GitHub => extract_github(html),
        Specialist::StackExchange => extract_stackexchange(html),
        Specialist::Arxiv => extract_arxiv(html),
        Specialist::Wikipedia => extract_wikipedia(html),
    }
    .map(|mut extract| {
        if extract.title.is_none() {
            extract.title = Some(url.path().trim_matches('/').to_string());
        }
        extract
    })
}

fn selector(query: &str) -> Option<Selector> {
    Selector::parse(query).ok()
}

fn text(element: &ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_text(document: &Html, query: &str) -> Option<String> {
    let selector = selector(query)?;
    document
        .select(&selector)
        .next()
        .map(|element| text(&element))
        .filter(|value| !value.is_empty())
}

fn elements_html(document: &Html, query: &str) -> Vec<String> {
    let Some(selector) = selector(query) else {
        return Vec::new();
    };
    document
        .select(&selector)
        .map(|element| element.inner_html())
        .collect()
}

fn html_to_markdown(html: &str) -> String {
    let cleaned = strip_non_content(html);
    htmd::convert(cleaned.as_ref()).unwrap_or_else(|_| {
        let document = Html::parse_fragment(cleaned.as_ref());
        document.root_element().text().collect::<String>()
    })
}

/// TemplateStyles and analytics snippets are nested inside otherwise useful
/// fragments (notably Wikipedia infobox tables). Converters render them as
/// plain text, so remove them before Markdown conversion.
fn strip_non_content(html: &str) -> Cow<'_, str> {
    static NON_CONTENT: OnceLock<Regex> = OnceLock::new();
    let regex = NON_CONTENT.get_or_init(|| {
        Regex::new(
            r"(?is)<style\b[^>]*>.*?</style\s*>|<script\b[^>]*>.*?</script\s*>|<noscript\b[^>]*>.*?</noscript\s*>",
        )
        .expect("non-content regex is valid")
    });
    regex.replace_all(html, "")
}

fn join_markdown(parts: Vec<String>) -> String {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for part in parts {
        let markdown = html_to_markdown(&part).trim().to_string();
        if !markdown.is_empty() && seen.insert(markdown.clone()) {
            output.push(markdown);
        }
    }
    output.join("\n\n---\n\n")
}

fn extract_github(html: &str) -> Option<Extract> {
    let document = Html::parse_document(html);
    let title = first_text(&document, "h1 bdi")
        .or_else(|| first_text(&document, "h1"))
        .or_else(|| first_text(&document, "title"));
    let mut bodies = elements_html(&document, ".comment-body");
    if bodies.is_empty() {
        bodies = elements_html(&document, ".markdown-body");
    }
    let markdown = join_markdown(bodies);
    if markdown.is_empty() {
        return None;
    }
    Some(Extract { title, markdown })
}

fn extract_stackexchange(html: &str) -> Option<Extract> {
    let document = Html::parse_document(html);
    let title = first_text(&document, "#question-header h1 a")
        .or_else(|| first_text(&document, "#question-header h1"))
        .or_else(|| first_text(&document, "h1 a"));
    let mut bodies = elements_html(&document, ".question .s-prose");
    if bodies.is_empty() {
        bodies = elements_html(&document, "#question .post-text");
    }
    let mut answers = elements_html(&document, ".answer .s-prose");
    if answers.is_empty() {
        answers = elements_html(&document, ".answer .post-text");
    }
    bodies.extend(answers);
    let markdown = join_markdown(bodies);
    if markdown.is_empty() {
        return None;
    }
    Some(Extract { title, markdown })
}

fn extract_arxiv(html: &str) -> Option<Extract> {
    let document = Html::parse_document(html);
    let title = first_text(&document, "h1.title")
        .map(|value| value.trim_start_matches("Title:").trim().to_string());
    let authors = first_text(&document, ".authors")
        .map(|value| value.trim_start_matches("Authors:").trim().to_string());
    let abstract_text = first_text(&document, "blockquote.abstract")
        .map(|value| value.trim_start_matches("Abstract:").trim().to_string());
    let abstract_text = abstract_text?;
    if abstract_text.is_empty() {
        return None;
    }
    let mut markdown = String::new();
    if let Some(title) = &title {
        markdown.push_str(&format!("# {title}\n\n"));
    }
    if let Some(authors) = &authors {
        markdown.push_str(&format!("**Authors:** {authors}\n\n"));
    }
    markdown.push_str("## Abstract\n\n");
    markdown.push_str(&abstract_text);
    Some(Extract {
        title,
        markdown,
    })
}

fn extract_wikipedia(html: &str) -> Option<Extract> {
    let document = Html::parse_document(html);
    let title = first_text(&document, "#firstHeading");
    let query = concat!(
        "#mw-content-text .mw-parser-output > p, ",
        "#mw-content-text .mw-parser-output > h2, ",
        "#mw-content-text .mw-parser-output > h3, ",
        "#mw-content-text .mw-parser-output > h4, ",
        "#mw-content-text .mw-parser-output > ul, ",
        "#mw-content-text .mw-parser-output > ol, ",
        "#mw-content-text .mw-parser-output > blockquote, ",
        "#mw-content-text .mw-parser-output > pre, ",
        "#mw-content-text .mw-parser-output > table, ",
        "#mw-content-text .mw-parser-output section > p, ",
        "#mw-content-text .mw-parser-output section > h2, ",
        "#mw-content-text .mw-parser-output section > h3, ",
        "#mw-content-text .mw-parser-output section > h4, ",
        "#mw-content-text .mw-parser-output section > ul, ",
        "#mw-content-text .mw-parser-output section > ol, ",
        "#mw-content-text .mw-parser-output section > blockquote, ",
        "#mw-content-text .mw-parser-output section > pre, ",
        "#mw-content-text .mw-parser-output section > table"
    );
    let parts = elements_html(&document, query);
    let markdown = join_markdown(parts);
    if markdown.is_empty() {
        return None;
    }
    Some(Extract { title, markdown })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_wikipedia_section_wrapped_paragraphs() {
        let html = r#"<html><body>
            <h1 id="firstHeading">Rust</h1>
            <div id="mw-content-text"><div class="mw-parser-output">
              <section><p>Rust is a general-purpose language.</p>
              <h2>History</h2>
              <p>It began in 2006.</p></section>
            </div></div>
        </body></html>"#;
        let url = url::Url::parse("https://en.wikipedia.org/wiki/Rust").unwrap();
        let extract = extract(Specialist::Wikipedia, html, &url).expect("extract");
        assert_eq!(extract.title.as_deref(), Some("Rust"));
        assert!(extract.markdown.contains("general-purpose language"));
        assert!(extract.markdown.contains("began in 2006"));
    }

    #[test]
    fn deduplicates_identical_markdown_blocks() {
        let markdown = join_markdown(vec![
            "<p>same</p>".into(),
            "<p>same</p>".into(),
            "<p>other</p>".into(),
        ]);
        assert_eq!(markdown.matches("same").count(), 1);
        assert!(markdown.contains("other"));
    }

    #[test]
    fn strips_template_styles_from_specialist_markdown() {
        let markdown = html_to_markdown(
            r#"<div><style data-mw="TemplateStyles">.mw-parser-output ul.cslist{color:red}</style><p>Visible text</p><script>track()</script></div>"#,
        );
        assert!(markdown.contains("Visible text"));
        assert!(!markdown.contains("cslist"));
        assert!(!markdown.contains("track()"));
    }
}
