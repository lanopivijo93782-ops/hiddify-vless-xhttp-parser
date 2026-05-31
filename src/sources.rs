//! Fetching subscription bodies from public sources and extracting the
//! `vless://` links inside them (handling base64-encoded subscriptions).

use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use futures::stream::{self, StreamExt};

const EMBEDDED_SOURCES: &str = include_str!("../data/sources.txt");

/// Parse the embedded default source list.
pub fn default_sources() -> Vec<String> {
    parse_source_list(EMBEDDED_SOURCES)
}

/// Parse a newline-separated source list, ignoring blanks and `#` comments.
pub fn parse_source_list(body: &str) -> Vec<String> {
    body.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

/// Result of fetching a single source.
pub struct FetchResult {
    pub url: String,
    pub body: Option<String>,
}

/// Fetch many sources concurrently. Failures are logged and skipped.
pub async fn fetch_all(
    client: &reqwest::Client,
    urls: &[String],
    concurrency: usize,
) -> Vec<FetchResult> {
    stream::iter(urls.iter().cloned())
        .map(|url| {
            let client = client.clone();
            async move {
                let body = match fetch_one(&client, &url).await {
                    Ok(b) => Some(b),
                    Err(e) => {
                        tracing::warn!(%url, error = %e, "source fetch failed");
                        None
                    }
                };
                FetchResult { url, body }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await
}

async fn fetch_one(client: &reqwest::Client, url: &str) -> Result<String> {
    let text = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(text)
}

/// Extract every `vless://` link from a subscription body, decoding base64
/// payloads when present.
pub fn extract_links(body: &str) -> Vec<String> {
    let decoded = maybe_base64_decode(body);
    let mut links = scan_for_vless(&decoded);

    // Some subscriptions are base64 only per-line; also scan the raw body.
    if links.is_empty() {
        links = scan_for_vless(body);
    }
    links
}

/// Find `vless://...` tokens in arbitrary text (whitespace separated).
fn scan_for_vless(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in text.split_whitespace() {
        if let Some(idx) = token.find("vless://") {
            out.push(token[idx..].to_string());
        }
    }
    out
}

/// If the whole body looks like base64 of text, decode it; otherwise return as-is.
fn maybe_base64_decode(body: &str) -> String {
    let compact: String = body.split_whitespace().collect();
    if compact.len() < 8 {
        return body.to_string();
    }
    // Subscriptions use standard or URL-safe base64, often without padding.
    let engines: [base64::engine::GeneralPurpose; 2] = [
        base64::engine::general_purpose::STANDARD_NO_PAD,
        base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ];
    let trimmed = compact.trim_end_matches('=');
    for engine in engines {
        if let Ok(bytes) = engine.decode(trimmed.as_bytes()) {
            if let Ok(text) = String::from_utf8(bytes) {
                if text.contains("://") {
                    return text;
                }
            }
        }
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_links() {
        let body = "vless://a@h:443?type=xhttp#x\nvless://b@h2:8443?type=tcp#y\n";
        let links = extract_links(body);
        assert_eq!(links.len(), 2);
        assert!(links[0].starts_with("vless://a@"));
    }

    #[test]
    fn extracts_base64_subscription() {
        let inner = "vless://a@h:443?type=xhttp#x\nvless://b@h2:8443?type=tcp#y";
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(inner);
        let links = extract_links(&encoded);
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn ignores_comments_in_source_list() {
        let list = "# comment\n\nhttps://a/b\nhttps://c/d\n";
        assert_eq!(parse_source_list(list).len(), 2);
    }

    #[test]
    fn embedded_sources_present() {
        assert!(default_sources().len() >= 8);
    }
}
