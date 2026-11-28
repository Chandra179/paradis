//! Bounded XML sitemap parser (`<urlset>` and `<sitemapindex>`) with DTD rejection.

use std::{error::Error, fmt};

use quick_xml::{Reader, events::Event};
use serde::Serialize;
use url::Url;

const MAX_XML_BYTES: usize = 2 * 1024 * 1024;
const MAX_XML_EVENTS: usize = 100_000;
const MAX_XML_DEPTH: usize = 64;
const MAX_SITEMAP_ENTRIES: usize = 500;

/// Kind of sitemap document parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SitemapKind {
    /// Standard `<urlset>` containing page URLs.
    UrlSet,
    /// `<sitemapindex>` pointing to child sitemaps.
    SitemapIndex,
}

/// A discovered URL entry from a sitemap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SitemapEntry {
    /// Sanitized HTTPS/HTTP URL (fragment stripped).
    pub url: String,
    /// Optional `<lastmod>` timestamp text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lastmod: Option<String>,
}

/// Parsed sitemap output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedSitemap {
    /// Whether this document is a `urlset` or `sitemapindex`.
    pub kind: SitemapKind,
    /// Discovered entries (capped at 500).
    pub entries: Vec<SitemapEntry>,
    /// Whether additional entries were omitted beyond the cap.
    pub truncated: bool,
}

/// Failure while parsing a sitemap XML payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitemapError {
    /// Input exceeds the 2 MiB size limit.
    TooLarge,
    /// XML contains a forbidden `DOCTYPE` declaration.
    DocTypeForbidden,
    /// XML exceeded nesting depth or event count budgets.
    BudgetExceeded,
    /// Malformed XML or missing `<urlset>` / `<sitemapindex>` root.
    MalformedXml,
}

impl fmt::Display for SitemapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("sitemap exceeds 2 MiB limit"),
            Self::DocTypeForbidden => f.write_str("sitemap XML must not contain a DOCTYPE"),
            Self::BudgetExceeded => f.write_str("sitemap XML exceeded nesting or event budget"),
            Self::MalformedXml => f.write_str("malformed sitemap XML"),
        }
    }
}

impl Error for SitemapError {}

/// Parses UTF-8 sitemap XML into discovery entries without network requests.
///
/// # Errors
///
/// Returns [`SitemapError`] on oversized input, DTDs, budget exhaustion, or malformed XML.
pub fn parse_sitemap(bytes: &[u8]) -> Result<ParsedSitemap, SitemapError> {
    if bytes.len() > MAX_XML_BYTES {
        return Err(SitemapError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| SitemapError::MalformedXml)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut kind: Option<SitemapKind> = None;
    let mut depth = 0usize;
    let mut events = 0usize;
    let mut in_entry = false;
    let mut current_tag: Option<&'static str> = None;
    let mut current_loc: Option<String> = None;
    let mut current_lastmod: Option<String> = None;
    let mut entries = Vec::new();
    let mut truncated = false;

    loop {
        events += 1;
        if events > MAX_XML_EVENTS {
            return Err(SitemapError::BudgetExceeded);
        }
        match reader.read_event() {
            Ok(Event::DocType(_)) => return Err(SitemapError::DocTypeForbidden),
            Ok(Event::Start(tag)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(SitemapError::BudgetExceeded);
                }
                let local = tag.local_name();
                match local.as_ref() {
                    "urlset" if depth == 1 && kind.is_none() => kind = Some(SitemapKind::UrlSet),
                    "sitemapindex" if depth == 1 && kind.is_none() => {
                        kind = Some(SitemapKind::SitemapIndex)
                    }
                    "url" if depth == 2 && kind == Some(SitemapKind::UrlSet) => in_entry = true,
                    "sitemap" if depth == 2 && kind == Some(SitemapKind::SitemapIndex) => {
                        in_entry = true
                    }
                    "loc" if in_entry && depth == 3 => current_tag = Some("loc"),
                    "lastmod" if in_entry && depth == 3 => current_tag = Some("lastmod"),
                    _ => {}
                }
            }
            Ok(Event::Text(text)) => {
                if let Some(field) = current_tag {
                    let decoded: &str = text.as_ref();
                    let trimmed = decoded.trim();
                    if !trimmed.is_empty() {
                        match field {
                            "loc" => current_loc = Some(trimmed.to_owned()),
                            "lastmod" => current_lastmod = Some(trimmed.to_owned()),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(tag)) => {
                if depth == 3 {
                    current_tag = None;
                } else if depth == 2 && in_entry {
                    in_entry = false;
                    if let Some(raw_url) = current_loc.take()
                        && let Some(clean_url) = sanitize_url(&raw_url)
                    {
                        if entries.len() < MAX_SITEMAP_ENTRIES {
                            entries.push(SitemapEntry {
                                url: clean_url,
                                lastmod: current_lastmod.take(),
                            });
                        } else {
                            truncated = true;
                        }
                    }
                    current_lastmod = None;
                }
                depth = depth.saturating_sub(1);
                let _ = tag;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(SitemapError::MalformedXml),
        }
    }

    let kind = kind.ok_or(SitemapError::MalformedXml)?;
    Ok(ParsedSitemap {
        kind,
        entries,
        truncated,
    })
}

fn sanitize_url(raw: &str) -> Option<String> {
    let mut parsed = Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    parsed.set_fragment(None);
    Some(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urlset_and_rejects_doctype() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.org/a#frag</loc><lastmod>2026-11-27</lastmod></url>
  <url><loc>https://user:pass@example.org/secret</loc></url>
</urlset>"#;
        let parsed = parse_sitemap(xml).unwrap();
        assert_eq!(parsed.kind, SitemapKind::UrlSet);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].url, "https://example.org/a");
        assert_eq!(parsed.entries[0].lastmod.as_deref(), Some("2026-11-27"));

        let dtd = br#"<!DOCTYPE urlset><urlset></urlset>"#;
        assert_eq!(parse_sitemap(dtd), Err(SitemapError::DocTypeForbidden));
    }
}
