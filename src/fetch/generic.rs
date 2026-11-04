use dom_smoothie::{Config, Readability};

/// A page body produced by one extraction level.
#[derive(Debug, Clone)]
pub struct Extract {
    pub title: Option<String>,
    pub markdown: String,
}

/// Generic readability extraction, then HTML-to-Markdown conversion.
pub fn extract(html: &str, document_url: &str) -> Option<Extract> {
    let config = Config {
        max_elements_to_parse: 20_000,
        ..Default::default()
    };
    let mut readability =
        Readability::new(html, Some(document_url), Some(config)).ok()?;
    let article = readability.parse().ok()?;
    let content_html = article.content.to_string();
    let markdown = htmd::convert(&content_html)
        .unwrap_or_else(|_| article.text_content.to_string());
    if markdown.trim().is_empty() {
        return None;
    }
    Some(Extract {
        title: non_empty(article.title),
        markdown,
    })
}

pub fn html_title(html: &str) -> Option<String> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("title").ok()?;
    document
        .select(&selector)
        .next()
        .map(|element| element.text().collect::<String>())
        .and_then(non_empty)
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}
