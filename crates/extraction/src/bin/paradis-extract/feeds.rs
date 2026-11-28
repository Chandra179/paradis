//! Bounded feed parsing and evidence for saved RSS and Atom snapshots.

use std::{collections::HashSet, error::Error, ffi::OsString, fs, io::Write, path::Path};

use extraction::{DocumentVersion, Extractor, InputFormat, sha256_hex};
use feed_rs::model::{Entry, Feed, FeedType, Link};
use quick_xml::{Reader, events::Event};
use reqwest::Url;
use serde::Serialize;
use time::OffsetDateTime;

use super::{
    Disposition, DocumentHistory, HackerNewsRecord, MAX_INPUT_BYTES, Record, SourceRecord, emit,
    emit_document,
    http::{self, Failure},
    read_snapshot, web,
};

const MAX_ENTRIES: usize = 100;
const MAX_LINKS_PER_ENTRY: usize = 20;
const MAX_XML_DEPTH: usize = 64;
const MAX_XML_EVENTS: usize = 100_000;
const HN_RSS: &str = "https://news.ycombinator.com/rss";
const HN_SOURCE: web::Source = web::Source {
    id: "hacker-news-rss-v1-title-only",
    host: "news.ycombinator.com",
    paths: &["/rss"],
    robots_paths: &["/robots.txt"],
    interval: http::REQUEST_INTERVAL,
    content: web::Content::Feed,
};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Rss2,
    Atom,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Field {
    Title,
    Summary,
    Content,
}

impl Field {
    fn version(self) -> &'static str {
        match self {
            Self::Title => "feed-title-v1",
            Self::Summary => "feed-summary-v1",
            Self::Content => "feed-content-v1",
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct FieldSource<'a> {
    locator: Field,
    entry_index: usize,
    item_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    published_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

pub(super) fn base_url(input: &str) -> Result<Url, &'static str> {
    let mut url = Url::parse(input).map_err(|_| "invalid feed base URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
    {
        return Err("feed base URL must be an HTTP(S) URL without credentials or a query");
    }
    url.set_fragment(None);
    Ok(url)
}

pub(super) fn extract_files(
    paths: impl IntoIterator<Item = OsString>,
    base: Option<&Url>,
    extractor: &Extractor,
    output: &mut impl Write,
) -> Result<(), Box<dyn Error>> {
    let mut history = DocumentHistory::default();
    for (index, argument) in paths.into_iter().enumerate() {
        let path = Path::new(&argument);
        let attempted_at = OffsetDateTime::now_utc();
        let attempt_index = index + 1;
        let loaded = (|| {
            let path = fs::canonicalize(path).map_err(|_| "unreadable_input")?;
            let source_id = path.to_str().ok_or("invalid_source_path")?.to_owned();
            let bytes = read_snapshot(&path, MAX_INPUT_BYTES)?;
            let (kind, feed) = parse(bytes.as_bytes(), base.map(Url::as_str))?;
            Ok((source_id, bytes, kind, feed))
        })();
        let (format, feed_id, entries_parsed, disposition, reason) = match &loaded {
            Ok((_, _, kind, feed)) if feed.entries.is_empty() => (
                Some(*kind),
                present(&feed.id),
                0,
                Disposition::Skipped,
                Some("no_feed_entries"),
            ),
            Ok((_, _, kind, feed)) => (
                Some(*kind),
                present(&feed.id),
                feed.entries.len(),
                Disposition::Accepted,
                None,
            ),
            Err(reason) => (None, None, 0, Disposition::Skipped, Some(*reason)),
        };
        emit(
            output,
            Record::FeedAttempt {
                attempt_index,
                requested_path: &path.to_string_lossy(),
                attempted_at,
                base_url: base.map(Url::as_str),
                format,
                feed_id,
                entries_parsed,
                entries_truncated: entries_parsed > MAX_ENTRIES,
                disposition,
                reason,
            },
        )?;
        if let Ok((source_id, bytes, kind, feed)) = loaded {
            emit_feed_links(
                output,
                attempt_index,
                None,
                feed.links
                    .iter()
                    .filter(|link| link.rel.as_deref() == Some("next")),
            )?;
            let digest = sha256_hex(bytes.as_bytes());
            let mut context = FeedOutput {
                attempt_index,
                source_id: &source_id,
                bytes: bytes.as_bytes(),
                digest: &digest,
                kind,
                extractor,
                history: &mut history,
                output,
            };
            for (position, entry) in feed.entries.iter().take(MAX_ENTRIES).enumerate() {
                context.entry(position + 1, entry)?;
            }
        }
    }
    Ok(())
}

struct FeedOutput<'a, W> {
    attempt_index: usize,
    source_id: &'a str,
    bytes: &'a [u8],
    digest: &'a str,
    kind: Kind,
    extractor: &'a Extractor,
    history: &'a mut DocumentHistory,
    output: &'a mut W,
}

impl<W: Write> FeedOutput<'_, W> {
    fn entry(&mut self, index: usize, entry: &Entry) -> Result<(), Box<dyn Error>> {
        let item_id = present(&entry.id);
        let fields = normalized_fields(entry);
        let empty = fields.is_empty();
        emit(
            self.output,
            Record::FeedItem {
                attempt_index: self.attempt_index,
                entry_index: index,
                item_id,
                disposition: if empty {
                    Disposition::Skipped
                } else {
                    Disposition::Accepted
                },
                reason: empty.then_some("no_usable_feed_fields"),
                fields_emitted: fields.len(),
            },
        )?;
        emit_feed_links(
            self.output,
            self.attempt_index,
            Some(index),
            entry.links.iter().chain(
                entry
                    .content
                    .iter()
                    .filter_map(|content| content.src.as_ref()),
            ),
        )?;
        let identity = match item_id {
            Some(id) => format!("id-{}", sha256_hex(id.as_bytes())),
            None => format!("snapshot-{}-{index}", self.digest),
        };
        for (field, text) in fields {
            let document = DocumentVersion::from_normalized(
                &format!(
                    "{}#feed-entry:{identity}:{}",
                    self.source_id,
                    field.version()
                ),
                self.bytes,
                field.version(),
                text,
            )?;
            let source_record = FieldSource {
                locator: field,
                entry_index: index,
                item_id,
                published_at: entry
                    .published
                    .as_ref()
                    .map(|date| date.timestamp().to_string()),
                // The RSS model may copy the publication time into the updated slot.
                updated_at: if matches!(self.kind, Kind::Atom) {
                    entry
                        .updated
                        .as_ref()
                        .map(|date| date.timestamp().to_string())
                } else {
                    None
                },
            };
            emit_document(
                self.output,
                self.attempt_index,
                &document,
                Some(SourceRecord::Feed(source_record)),
                &self.extractor.extract(&document)?,
                self.history.observe(self.attempt_index, &document),
            )?;
        }
        Ok(())
    }
}

fn emit_feed_links<'a>(
    output: &mut impl Write,
    attempt_index: usize,
    entry_index: Option<usize>,
    links: impl Iterator<Item = &'a Link>,
) -> Result<(), Box<dyn Error>> {
    for link in links.take(MAX_LINKS_PER_ENTRY) {
        let url = discovery_url(&link.href);
        emit(
            output,
            Record::FeedLink {
                attempt_index,
                entry_index,
                relation: match link.rel.as_deref() {
                    None => "alternate",
                    Some(
                        relation @ ("alternate" | "related" | "self" | "enclosure" | "via" | "next"
                        | "prev" | "previous"),
                    ) => relation,
                    Some(_) => "other",
                },
                url: url.as_ref().ok().map(Url::as_str),
                reason: url.as_ref().err().copied(),
            },
        )?;
    }
    Ok(())
}

fn discovery_url(input: &str) -> Result<Url, &'static str> {
    let mut url = Url::parse(input).map_err(|error| {
        if matches!(error, url::ParseError::RelativeUrlWithoutBase) {
            "unresolved_relative_url"
        } else {
            "invalid_url"
        }
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("unsupported_link_scheme");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("link_credentials_withheld");
    }
    if url.query().is_some() {
        return Err("link_query_withheld");
    }
    url.set_fragment(None);
    Ok(url)
}

fn normalized_fields(entry: &Entry) -> Vec<(Field, String)> {
    [
        (
            Field::Title,
            entry
                .title
                .as_ref()
                .map(|value| (value.content.as_str(), value.content_type.as_str())),
        ),
        (
            Field::Summary,
            entry
                .summary
                .as_ref()
                .map(|value| (value.content.as_str(), value.content_type.as_str())),
        ),
        (
            Field::Content,
            entry.content.as_ref().and_then(|value| {
                value
                    .body
                    .as_deref()
                    .map(|body| (body, value.content_type.as_str()))
            }),
        ),
    ]
    .into_iter()
    .filter_map(|(field, value)| {
        let (text, media_type) = value?;
        normalize_field(text, media_type).map(|text| (field, text))
    })
    .collect()
}

fn normalize_field(text: &str, media_type: &str) -> Option<String> {
    let input = match media_type.split(';').next()?.trim() {
        "text/plain" => InputFormat::Text,
        "text/html" | "application/xhtml+xml" => InputFormat::Html,
        _ => return None,
    };
    input
        .normalize_text(text)
        .ok()
        .filter(|text| !text.is_empty())
}

fn present(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn parse(bytes: &[u8], base: Option<&str>) -> Result<(Kind, Feed), &'static str> {
    let input = std::str::from_utf8(bytes).map_err(|_| "invalid_utf8")?;
    validate_xml(input)?;
    let feed = feed_rs::parser::Builder::new()
        .base_uri(base)
        .id_generator(|_, _, _| String::new())
        .build()
        .parse(bytes)
        .map_err(|_| "invalid_feed")?;
    let kind = match feed.feed_type {
        FeedType::RSS2 => Kind::Rss2,
        FeedType::Atom => Kind::Atom,
        _ => return Err("unsupported_feed_format"),
    };
    Ok((kind, feed))
}

fn validate_xml(input: &str) -> Result<(), &'static str> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().check_comments = true;
    let mut depth = 0;
    let mut root = false;
    for _ in 0..MAX_XML_EVENTS {
        let event = reader.read_event().map_err(|_| "invalid_feed_xml")?;
        match event {
            Event::DocType(_) => return Err("xml_doctype_disallowed"),
            Event::Start(ref element) | Event::Empty(ref element) => {
                if depth >= MAX_XML_DEPTH {
                    return Err("xml_depth_limit");
                }
                if depth == 0 {
                    if root {
                        return Err("invalid_feed_xml");
                    }
                    if !matches!(element.local_name().as_ref(), "rss" | "feed") {
                        return Err("unsupported_feed_format");
                    }
                    root = true;
                }
                for attribute in element.attributes() {
                    attribute
                        .map_err(|_| "invalid_feed_xml")?
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| "invalid_feed_xml")?;
                }
                depth += usize::from(matches!(event, Event::Start(_)));
            }
            Event::End(_) if depth != 0 => depth -= 1,
            Event::End(_) => return Err("invalid_feed_xml"),
            Event::Decl(declaration) => {
                if root {
                    return Err("invalid_feed_xml");
                }
                if let Some(encoding) = declaration.encoding() {
                    let encoding = encoding.map_err(|_| "invalid_feed_xml")?;
                    if !encoding.eq_ignore_ascii_case("utf-8") {
                        return Err("unsupported_xml_encoding");
                    }
                }
            }
            Event::GeneralRef(reference) => {
                if depth == 0 {
                    return Err("invalid_feed_xml");
                }
                if !matches!(reference.as_ref(), "amp" | "lt" | "gt" | "apos" | "quot")
                    && reference
                        .resolve_char_ref()
                        .map_err(|_| "invalid_feed_xml")?
                        .is_none()
                {
                    return Err("unsupported_xml_entity");
                }
            }
            Event::Text(text)
                if depth == 0
                    && !text
                        .as_ref()
                        .chars()
                        .all(|character| matches!(character, ' ' | '\n' | '\r' | '\t')) =>
            {
                return Err("invalid_feed_xml");
            }
            Event::CData(_) if depth == 0 => return Err("invalid_feed_xml"),
            Event::Eof => return (root && depth == 0).then_some(()).ok_or("invalid_feed_xml"),
            _ => {}
        }
    }
    Err("xml_event_limit")
}

pub(super) fn extract_hacker_news(
    limit: usize,
    extractor: &Extractor,
    output: &mut impl Write,
) -> Result<(), Box<dyn Error>> {
    let (attempt_index, rows) = web::acquire(&HN_SOURCE, HN_RSS, output, |bytes| {
        hacker_news_titles(bytes, limit)
    })?;
    let mut history = DocumentHistory::default();
    for (document, source) in rows {
        emit_document(
            output,
            attempt_index,
            &document,
            Some(SourceRecord::HackerNews(source)),
            &extractor.extract(&document)?,
            history.observe(attempt_index, &document),
        )?;
    }
    Ok(())
}

fn hacker_news_titles(
    bytes: &[u8],
    limit: usize,
) -> Result<Vec<(DocumentVersion, HackerNewsRecord)>, Failure> {
    let (kind, feed) = parse(bytes, Some(HN_RSS)).map_err(Failure::skip)?;
    if !matches!(kind, Kind::Rss2) {
        return Err(Failure::skip("unexpected_feed_format"));
    }
    let comments = rss_comments(bytes).map_err(Failure::skip)?;
    if comments.len() != feed.entries.len() {
        return Err(Failure::skip("rss_entry_alignment_failed"));
    }
    let mut seen = HashSet::new();
    let mut titles = Vec::new();
    for (index, (item, comments_id)) in feed
        .entries
        .iter()
        .zip(comments)
        .take(MAX_ENTRIES)
        .enumerate()
    {
        let (id, id_field) = match (story_id(&item.id), comments_id) {
            (Some(guid), Some(comments)) if guid != comments => continue,
            (_, Some(comments)) => (comments, "comments"),
            (Some(guid), None) => (guid, "guid"),
            _ => continue,
        };
        let Some(title) = item
            .title
            .as_ref()
            .and_then(|value| normalize_field(&value.content, value.content_type.as_str()))
        else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        let document = DocumentVersion::from_normalized(
            &format!("{HN_RSS}#story-{id}"),
            bytes,
            "hn-rss-title-v1",
            title,
        )
        .map_err(|_| Failure::skip("invalid_document_version"))?;
        titles.push((
            document,
            HackerNewsRecord {
                id,
                locator: format!("rss.channel.item[{}].title", index + 1).into(),
                id_locator: Some(format!("rss.channel.item[{}].{id_field}", index + 1)),
                published_at: None,
            },
        ));
        if titles.len() == limit {
            break;
        }
    }
    (!titles.is_empty())
        .then_some(titles)
        .ok_or_else(|| Failure::skip("no_story_titles_found"))
}

fn rss_comments(bytes: &[u8]) -> Result<Vec<Option<u64>>, &'static str> {
    // The feed model merges identical comment and item URLs. Read the declared
    // comments field itself so a link to another story cannot become this item's ID.
    let mut reader = Reader::from_str(std::str::from_utf8(bytes).map_err(|_| "invalid_utf8")?);
    let mut path = Vec::<String>::new();
    let mut ids = Vec::new();
    let mut comments_seen = false;
    loop {
        match reader.read_event().map_err(|_| "invalid_feed_xml")? {
            Event::Start(element) => {
                let name = element.name();
                let tag = name.as_ref();
                if tag == "comments" && path == ["rss", "channel", "item"] {
                    let value = reader.read_text(name).map_err(|_| "invalid_feed_xml")?;
                    if let Some(id) = ids.last_mut() {
                        *id = if comments_seen {
                            None
                        } else {
                            quick_xml::escape::unescape(&value)
                                .ok()
                                .and_then(|text| story_id(text.trim()))
                        };
                    }
                    comments_seen = true;
                    continue;
                }
                if tag == "item" && path == ["rss", "channel"] {
                    ids.push(None);
                    comments_seen = false;
                }
                path.push(tag.to_owned());
            }
            Event::Empty(element) => match element.name().as_ref() {
                "item" if path == ["rss", "channel"] => ids.push(None),
                "comments" if path == ["rss", "channel", "item"] => {
                    if let Some(id) = ids.last_mut() {
                        *id = None;
                    }
                    comments_seen = true;
                }
                _ => {}
            },
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
    }
}

fn story_id(locator: &str) -> Option<u64> {
    let url = Url::parse(locator).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some(HN_SOURCE.host)
        || url.port().is_some()
        || url.path() != "/item"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let id = url.query()?.strip_prefix("id=")?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    id.parse::<u64>().ok().filter(|id| *id > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hacker_news_uses_declared_story_ids_and_retains_titles_only() {
        let payload = br#"<rss version="2.0"><channel><title>Hacker News</title>
          <item><title>The cabinet uses 10 kWh</title>
            <link>https://news.ycombinator.com/item?id=11</link>
            <comments>https://news.ycombinator.com/item?id=11</comments>
            <description>private description</description>
          </item>
          <item><title>A story about a previous post</title>
            <link>https://news.ycombinator.com/item?id=11</link>
            <comments>https://news.ycombinator.com/item?id=21</comments>
          </item>
          <item><title>An unrelated link</title><link>https://news.ycombinator.com/item?id=31</link></item>
          <item><title>Conflicting IDs</title><guid>https://news.ycombinator.com/item?id=41</guid>
            <comments>https://news.ycombinator.com/item?id=42</comments>
          </item>
          <item><title>Explicit GUID</title><guid>https://news.ycombinator.com/item?id=51</guid></item>
          </channel></rss>"#;
        let rows = hacker_news_titles(payload, 10).unwrap();
        let ids: Vec<_> = rows.iter().map(|(_, record)| record.id).collect();
        assert_eq!(ids, [11, 21, 51]);
        assert_eq!(rows[0].0.text(), "The cabinet uses 10 kWh");
        assert_eq!(
            rows[0].1.id_locator.as_deref(),
            Some("rss.channel.item[1].comments")
        );
        assert_eq!(
            rows[2].1.id_locator.as_deref(),
            Some("rss.channel.item[5].guid")
        );
        let records = serde_json::to_string(&rows).unwrap();
        assert!(!records.contains("private description"));
        assert!(!records.contains("item?id="));
    }
}
