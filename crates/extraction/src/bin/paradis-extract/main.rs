//! Reads saved snapshots or approved live sources and emits JSON Lines.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod feeds;
mod hacker_news;
mod hacker_news_page;
mod http;
mod provenance;
mod review;
mod snapshots;
mod web;

use std::{
    env,
    error::Error,
    fs::{self, File},
    io::{self, BufWriter, Read, Write},
    path::Path,
};

use extraction::{
    Candidate, DependencyParse, DocumentVersion, Extraction, Extractor, InputFormat, PageDecision,
    PageScreening,
};
use serde::Serialize;
use time::OffsetDateTime;

use provenance::{DeclaredLinks, DocumentHistory, DocumentReference, Provenance};

const MAX_INPUT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PARSE_BYTES: u64 = 16 * 1024 * 1024;
const HELP: &str = "Usage: paradis-extract [--html-body] [--] FILE...\n       paradis-extract [--html-body] --dependencies PARSE [--] FILE\n       paradis-extract --snapshot-manifest MANIFEST\n       paradis-extract --feed [--base-url URL] [--] FILE...\n       paradis-extract --sitemap FILE\n       paradis-extract --hacker-news [LIMIT]\n       paradis-extract --hacker-news-rss [LIMIT]\n       paradis-extract --hacker-news-page [LIMIT]\n\nRead saved UTF-8 text, HTML, or RSS 2.0 and Atom feeds (up to 2 MiB per file).\nUse --html-body to compare HTML extraction without main-content pruning.\nUse --dependencies for an offline relationship comparison on one file.\nUse --snapshot-manifest for saved versions and an optional evidence review.\nUse --sitemap to parse a saved XML sitemap into URL discovery records.\nFor Hacker News, use the RSS feed or official API and compare with the front page (default 10, maximum 25).\nWrite attempts, documents, and candidates as JSON Lines.\n";

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1).peekable();
    if matches!(
        args.peek().and_then(|arg| arg.to_str()),
        Some("-h" | "--help")
    ) {
        print!("{HELP}");
        return Ok(());
    }
    if args.peek().is_none() {
        return Err(HELP.into());
    }

    let extractor =
        Extractor::new().map_err(|error| format!("compile extraction patterns: {error}"))?;
    let mut output = BufWriter::new(io::stdout().lock());
    let result = if args.peek().is_some_and(|arg| {
        arg == "--hacker-news" || arg == "--hacker-news-page" || arg == "--hacker-news-rss"
    }) {
        let mode = args.next().ok_or(HELP)?;
        let limit = match args.next() {
            None => hacker_news::DEFAULT_LIMIT,
            Some(arg) => arg
                .to_str()
                .and_then(|arg| arg.parse::<usize>().ok())
                .filter(|value| (1..=hacker_news::MAX_LIMIT).contains(value))
                .ok_or("Hacker News limit must be between 1 and 25")?,
        };
        if args.next().is_some() {
            return Err(HELP.into());
        }
        if mode == "--hacker-news-page" {
            hacker_news_page::extract(limit, &extractor, &mut output)
        } else if mode == "--hacker-news-rss" {
            feeds::extract_hacker_news(limit, &extractor, &mut output)
        } else {
            hacker_news::extract(limit, &extractor, &mut output)
        }
    } else if args.peek().is_some_and(|arg| arg == "--feed") {
        args.next();
        let base = if args.peek().is_some_and(|arg| arg == "--base-url") {
            args.next();
            let url = args.next().ok_or(HELP)?;
            Some(feeds::base_url(
                url.to_str().ok_or("feed base URL must be UTF-8")?,
            )?)
        } else {
            None
        };
        if args.peek().is_some_and(|arg| arg == "--") {
            args.next();
        }
        if args.peek().is_none() || (base.is_some() && args.len() != 1) {
            return Err(HELP.into());
        }
        feeds::extract_files(args, base.as_ref(), &extractor, &mut output)
    } else if args.peek().is_some_and(|arg| arg == "--sitemap") {
        args.next();
        let path = args.next().ok_or(HELP)?;
        if args.next().is_some() {
            return Err(HELP.into());
        }
        let raw = read_snapshot(Path::new(&path), MAX_INPUT_BYTES)
            .map_err(|reason| format!("read sitemap: {reason}"))?;
        let parsed = extraction::sitemap::parse_sitemap(raw.as_bytes())?;
        for entry in &parsed.entries {
            emit(
                &mut output,
                Record::FeedLink {
                    attempt_index: 1,
                    entry_index: None,
                    relation: match parsed.kind {
                        extraction::sitemap::SitemapKind::UrlSet => "sitemap_url",
                        extraction::sitemap::SitemapKind::SitemapIndex => "sitemap_child",
                    },
                    url: Some(&entry.url),
                    reason: None,
                },
            )?;
        }
        Ok(())
    } else if args.peek().is_some_and(|arg| arg == "--snapshot-manifest") {
        args.next();
        let path = args.next().ok_or(HELP)?;
        if args.next().is_some() {
            return Err(HELP.into());
        }
        snapshots::run(Path::new(&path), &extractor, &mut output)
    } else {
        let html_format = if args.peek().is_some_and(|arg| arg == "--html-body") {
            args.next();
            InputFormat::HtmlBody
        } else {
            InputFormat::Html
        };
        let parse_path = if args.peek().is_some_and(|arg| arg == "--dependencies") {
            args.next();
            Some(args.next().ok_or(HELP)?)
        } else {
            None
        };
        if args.peek().is_some_and(|arg| arg == "--") {
            args.next();
        }
        if args.peek().is_none() || (parse_path.is_some() && args.len() != 1) {
            return Err(HELP.into());
        }
        let dependencies = parse_path
            .map(|path| -> Result<DependencyParse, Box<dyn Error>> {
                let annotations = read_snapshot(Path::new(&path), MAX_PARSE_BYTES)
                    .map_err(|reason| format!("read dependency annotations: {reason}"))?;
                Ok(serde_json::from_str(&annotations)?)
            })
            .transpose()?;
        let mut history = DocumentHistory::default();
        for (index, arg) in args.enumerate() {
            drop(extract_file(
                SavedInput {
                    path: Path::new(&arg),
                    snapshot: None,
                    links: DeclaredLinks::default(),
                },
                index + 1,
                html_format,
                &extractor,
                dependencies.as_ref(),
                &mut history,
                &mut output,
            )?);
        }
        Ok(())
    };
    output.flush()?;
    result
}

struct SavedInput<'a> {
    path: &'a Path,
    snapshot: Option<&'a snapshots::Snapshot>,
    links: DeclaredLinks,
}

struct SavedResult {
    reference: DocumentReference,
    extraction: Extraction,
}

fn extract_file(
    input: SavedInput<'_>,
    attempt_index: usize,
    html_format: InputFormat,
    extractor: &Extractor,
    dependencies: Option<&DependencyParse>,
    history: &mut DocumentHistory,
    output: &mut impl Write,
) -> Result<Option<SavedResult>, Box<dyn Error>> {
    let SavedInput {
        path,
        snapshot,
        mut links,
    } = input;
    let requested_path = path.to_string_lossy();
    let attempted_at = OffsetDateTime::now_utc();
    let format = path
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(|extension| match extension.to_ascii_lowercase().as_str() {
            "html" | "htm" => Some(html_format),
            "txt" => Some(InputFormat::Text),
            _ => None,
        });
    let result = (|| {
        if let Some(reason) = snapshot.and_then(|entry| entry.withhold_reason(attempted_at)) {
            return Err(reason);
        }
        let format = format.ok_or("unsupported_format")?;
        let path = fs::canonicalize(path).map_err(|_| "unreadable_input")?;
        let source_id = snapshot
            .map(|entry| entry.source_id.as_str())
            .or_else(|| path.to_str())
            .ok_or("invalid_source_path")?;
        let bytes = read_snapshot(&path, MAX_INPUT_BYTES)?;
        let inspection = format
            .inspect(source_id, &bytes)
            .map_err(|_| "normalization_failed")?;
        if let Some(reason) =
            snapshot.and_then(|entry| entry.withhold_reason(OffsetDateTime::now_utc()))
        {
            return Err(reason);
        }
        Ok(inspection)
    })();
    let (disposition, reason, document, page_screening, metadata) = match &result {
        Ok(inspection) => {
            let document = inspection.document.as_ref();
            let screening = inspection.page_screening.as_ref();
            let metadata = inspection.metadata.as_ref();
            let decision = screening.map(PageScreening::decision);
            let (disposition, reason) = match decision {
                Some(PageDecision::Skipped(reason)) => (Disposition::Skipped, Some(reason)),
                Some(PageDecision::Uncertain(reason)) => (Disposition::Uncertain, Some(reason)),
                _ if document.is_some() => (Disposition::Accepted, None),
                _ => (Disposition::Skipped, Some("empty_text")),
            };
            (disposition, reason, document, screening, metadata)
        }
        Err(reason) => (Disposition::Skipped, Some(*reason), None, None, None),
    };
    emit(
        output,
        Record::FileAttempt {
            attempt_index,
            requested_path: &requested_path,
            attempted_at,
            format,
            disposition,
            reason,
            document_version_id: document.map(DocumentVersion::id),
            page_screening,
            metadata,
            snapshot,
        },
    )?;

    if let Some(document) = document {
        let extraction = match dependencies {
            Some(dependencies) => extractor
                .extract_with_dependencies(document, dependencies)
                .map_err(|error| format!("dependency extraction: {error}"))?,
            None => extractor.extract(document)?,
        };
        links.supersedes_version = links
            .supersedes_version
            .filter(|parent| !parent.matches(document.id()));
        let mut provenance = history.observe(attempt_index, document);
        provenance.declared = links;
        emit_document(
            output,
            attempt_index,
            document,
            None,
            &extraction,
            provenance,
        )?;
        return Ok(Some(SavedResult {
            reference: DocumentReference::new(attempt_index, document.id()),
            extraction,
        }));
    }
    Ok(None)
}

fn emit_document(
    output: &mut impl Write,
    attempt_index: usize,
    document: &DocumentVersion,
    source_record: Option<SourceRecord<'_>>,
    extraction: &Extraction,
    provenance: Provenance,
) -> Result<(), Box<dyn Error>> {
    emit(
        output,
        Record::Document {
            attempt_index,
            document,
            source_record,
            provenance,
            candidate_count: extraction.candidates.len(),
            truncated: extraction.truncated,
        },
    )?;
    for candidate in &extraction.candidates {
        emit(
            output,
            Record::Candidate {
                attempt_index,
                candidate,
            },
        )?;
    }
    Ok(())
}

fn read_snapshot(path: &Path, limit: u64) -> Result<String, &'static str> {
    if !fs::metadata(path)
        .map_err(|_| "unreadable_input")?
        .is_file()
    {
        return Err("not_regular_file");
    }
    let file = File::open(path).map_err(|_| "unreadable_input")?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable_input")?;
    if bytes.len() as u64 > limit {
        return Err("input_too_large");
    }
    String::from_utf8(bytes).map_err(|_| "invalid_utf8")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Disposition {
    Accepted,
    Skipped,
    Restricted,
    Deferred,
    Uncertain,
    Redirected,
}

#[derive(Serialize)]
struct HackerNewsRecord {
    id: u64,
    locator: std::borrow::Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id_locator: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    published_at: Option<OffsetDateTime>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum SourceRecord<'a> {
    HackerNews(HackerNewsRecord),
    Feed(feeds::FieldSource<'a>),
}

#[derive(Serialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
enum Record<'a> {
    #[serde(rename = "attempt")]
    FileAttempt {
        attempt_index: usize,
        requested_path: &'a str,
        #[serde(with = "time::serde::rfc3339")]
        attempted_at: OffsetDateTime,
        format: Option<InputFormat>,
        disposition: Disposition,
        reason: Option<&'static str>,
        document_version_id: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        page_screening: Option<&'a PageScreening>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<&'a extraction::DocumentMetadata>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot: Option<&'a snapshots::Snapshot>,
    },
    RequestAttempt {
        attempt_index: usize,
        requested_url: &'a str,
        final_url: Option<&'a str>,
        discovered_from: Option<&'a str>,
        #[serde(with = "time::serde::rfc3339")]
        attempted_at: OffsetDateTime,
        access_policy: &'static str,
        http_status: Option<u16>,
        retry_after: Option<&'a str>,
        disposition: Disposition,
        reason: Option<&'static str>,
        document_version_id: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_kind: Option<web::Purpose>,
        #[serde(skip_serializing_if = "Option::is_none")]
        robots_decision: Option<&'a web::RobotsDecision>,
    },
    FeedAttempt {
        attempt_index: usize,
        requested_path: &'a str,
        #[serde(with = "time::serde::rfc3339")]
        attempted_at: OffsetDateTime,
        base_url: Option<&'a str>,
        format: Option<feeds::Kind>,
        feed_id: Option<&'a str>,
        entries_parsed: usize,
        entries_truncated: bool,
        disposition: Disposition,
        reason: Option<&'static str>,
    },
    FeedItem {
        attempt_index: usize,
        entry_index: usize,
        item_id: Option<&'a str>,
        disposition: Disposition,
        reason: Option<&'static str>,
        fields_emitted: usize,
    },
    FeedLink {
        attempt_index: usize,
        entry_index: Option<usize>,
        relation: &'a str,
        url: Option<&'a str>,
        reason: Option<&'static str>,
    },
    Document {
        attempt_index: usize,
        document: &'a DocumentVersion,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_record: Option<SourceRecord<'a>>,
        #[serde(flatten)]
        provenance: Provenance,
        candidate_count: usize,
        truncated: bool,
    },
    Candidate {
        attempt_index: usize,
        candidate: &'a Candidate,
    },
    EvidenceReview {
        #[serde(flatten)]
        review: &'a review::Report<'a>,
    },
}

fn emit(output: &mut impl Write, record: Record<'_>) -> Result<(), Box<dyn Error>> {
    serde_json::to_writer(&mut *output, &record)?;
    output.write_all(b"\n")?;
    Ok(())
}
