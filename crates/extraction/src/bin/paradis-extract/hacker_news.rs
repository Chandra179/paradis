//! Bounded client for the official Hacker News API.

use std::{collections::HashSet, error::Error, io::Write, thread};

use extraction::{DocumentVersion, Extractor, InputFormat};
use reqwest::blocking::Client;
use serde::{Deserialize, de::DeserializeOwned};
use time::OffsetDateTime;

use super::{
    Disposition, DocumentHistory, HackerNewsRecord, Record, SourceRecord, emit, emit_document,
    http::{self, Failure, REQUEST_INTERVAL, check_status},
};

const HOST: &str = "hacker-news.firebaseio.com";
const TOP_STORIES: &str = "https://hacker-news.firebaseio.com/v0/topstories.json";
const ACCESS_POLICY: &str = "hacker-news-public-api-v1-title-only";
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;
/// Default number of top item IDs to inspect.
pub(super) const DEFAULT_LIMIT: usize = 10;
/// Maximum number of item requests allowed per run.
pub(super) const MAX_LIMIT: usize = 25;

/// Queries top stories and emits candidates from their normalized titles.
pub(super) fn extract(
    limit: usize,
    extractor: &Extractor,
    output: &mut impl Write,
) -> Result<(), Box<dyn Error>> {
    let client = http::client(HOST)?;

    let (index_attempt, index_result) = get_json::<Vec<u64>>(&client, TOP_STORIES);
    let item_ids = report_index_attempt(&index_attempt, index_result, output)?;

    let mut seen = HashSet::new();
    let mut history = DocumentHistory::default();
    for (position, id) in item_ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .take(limit)
        .enumerate()
    {
        thread::sleep(REQUEST_INTERVAL);
        let attempt_index = position + 2;
        let url = format!("https://{HOST}/v0/item/{id}.json");
        let (attempt, response) = get_json::<Option<Item>>(&client, &url);
        match response.and_then(|payload| story(id, &url, payload)) {
            Ok((document, source_record)) => {
                attempt.emit(output, attempt_index, Ok(()), Some(document.id()))?;
                emit_document(
                    output,
                    attempt_index,
                    &document,
                    Some(SourceRecord::HackerNews(source_record)),
                    &extractor.extract(&document)?,
                    history.observe(attempt_index, &document),
                )?;
            }
            Err(failure) => {
                attempt.emit(output, attempt_index, Err(failure), None)?;
                if matches!(
                    failure.disposition,
                    Disposition::Deferred | Disposition::Restricted
                ) {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn report_index_attempt(
    attempt: &Attempt<'_>,
    result: Result<Payload<Vec<u64>>, Failure>,
    output: &mut impl Write,
) -> Result<Vec<u64>, Box<dyn Error>> {
    attempt.emit(
        output,
        1,
        result.as_ref().map(|_| ()).map_err(|failure| *failure),
        None,
    )?;
    result
        .map(|payload| payload.value)
        .map_err(|failure| format!("Hacker News index request failed ({})", failure.reason).into())
}

#[derive(Deserialize)]
struct Item {
    id: u64,
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    time: Option<i64>,
    #[serde(default)]
    dead: bool,
    #[serde(default)]
    deleted: bool,
}

fn story(
    id: u64,
    url: &str,
    payload: Payload<Option<Item>>,
) -> Result<(DocumentVersion, HackerNewsRecord), Failure> {
    let item = payload.value.ok_or(Failure::skip("item_missing"))?;
    if item.id != id {
        return Err(Failure::skip("unexpected_item_id"));
    }
    if item.dead || item.deleted {
        return Err(Failure::skip("item_unavailable"));
    }
    if item.kind.as_deref() != Some("story") {
        return Err(Failure::skip("unsupported_item_type"));
    }
    let title = item.title.ok_or(Failure::skip("missing_title"))?;
    let text = InputFormat::Html
        .normalize_text(&title)
        .map_err(|_| Failure::skip("normalization_failed"))?;
    if text.is_empty() {
        return Err(Failure::skip("empty_text"));
    }
    let document = DocumentVersion::from_normalized(url, &payload.bytes, "hn-title-v1", text)
        .map_err(|_| Failure::skip("invalid_document_version"))?;
    let source_record = HackerNewsRecord {
        id,
        locator: "/title".into(),
        id_locator: None,
        published_at: item
            .time
            .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok()),
    };
    Ok((document, source_record))
}

struct Payload<T> {
    value: T,
    bytes: Vec<u8>,
}

struct Attempt<'a> {
    url: &'a str,
    time: OffsetDateTime,
    status: Option<u16>,
    retry_after: Option<String>,
}

impl Attempt<'_> {
    fn emit(
        &self,
        output: &mut impl Write,
        attempt_index: usize,
        result: Result<(), Failure>,
        document_version_id: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        let (disposition, reason) = match result {
            Ok(()) => (Disposition::Accepted, None),
            Err(failure) => (failure.disposition, Some(failure.reason)),
        };
        emit(
            output,
            Record::RequestAttempt {
                attempt_index,
                requested_url: self.url,
                final_url: self.status.map(|_| self.url),
                discovered_from: (attempt_index > 1).then_some(TOP_STORIES),
                attempted_at: self.time,
                access_policy: ACCESS_POLICY,
                http_status: self.status,
                retry_after: self.retry_after.as_deref(),
                disposition,
                reason,
                document_version_id,
                request_kind: None,
                robots_decision: None,
            },
        )
    }
}

fn get_json<'a, T: DeserializeOwned>(
    client: &Client,
    url: &'a str,
) -> (Attempt<'a>, Result<Payload<T>, Failure>) {
    let mut attempt = Attempt {
        url,
        time: OffsetDateTime::now_utc(),
        status: None,
        retry_after: None,
    };
    let result = http::get(client, url, "application/json").and_then(|reply| {
        attempt.status = Some(reply.status);
        attempt.retry_after = reply.retry_after();
        check_status(reply.status, reply.challenge())?;
        let bytes = http::read_body(reply, &["application/json"], MAX_RESPONSE_BYTES)?;
        let value = serde_json::from_slice(&bytes).map_err(|_| Failure::skip("invalid_json"))?;
        Ok(Payload { value, bytes })
    });
    (attempt, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_failure_reports_an_attempt_and_returns_an_error() {
        let attempt = Attempt {
            url: TOP_STORIES,
            time: OffsetDateTime::UNIX_EPOCH,
            status: None,
            retry_after: None,
        };
        let mut output = Vec::new();
        let failure = Failure::skip("request_timeout");
        let error = report_index_attempt(&attempt, Err(failure), &mut output).unwrap_err();
        assert!(error.to_string().contains("request_timeout"));
        let row: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(row["record_type"], "request_attempt");
        assert_eq!(row["attempt_index"], 1);
        assert_eq!(row["reason"], "request_timeout");
    }

    #[test]
    fn records_terminal_http_dispositions() {
        assert!(check_status(200, false).is_ok());
        for (status, challenge, reason) in [
            (200, true, "access_challenge"),
            (403, false, "access_restricted"),
            (429, false, "rate_limited"),
            (503, false, "service_unavailable"),
            (302, false, "redirect_or_revalidation"),
            (206, false, "partial_content"),
        ] {
            assert_eq!(
                check_status(status, challenge)
                    .err()
                    .map(|failure| failure.reason),
                Some(reason)
            );
        }
    }

    #[test]
    fn emits_only_the_normalized_story_title() {
        let bytes = br#"{"id":7,"type":"story","title":"Caf&eacute; contains 20 kg of supplies","time":1000,"by":"hidden-name","text":"hidden-body","url":"https://example.com"}"#.to_vec();
        let Ok(value) = serde_json::from_slice(&bytes) else {
            panic!("fixture must be valid JSON")
        };
        let Ok((document, source)) = story(
            7,
            "https://hacker-news.firebaseio.com/v0/item/7.json",
            Payload { value, bytes },
        ) else {
            panic!("fixture must contain an available story")
        };
        assert_eq!(document.text(), "Café contains 20 kg of supplies");
        assert_eq!(source.locator, "/title");
        let Ok(json) = serde_json::to_string(&document) else {
            panic!("document must serialize")
        };
        for forbidden in ["hidden-name", "hidden-body", "example.com"] {
            assert!(!json.contains(forbidden));
        }
    }
}
