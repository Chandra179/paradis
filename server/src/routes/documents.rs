//! Document extraction, listing, detail inspection, and evidence review endpoints.

use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use extraction::{
    CandidateValue, DocumentMetadata, Extractor, InputFormat, PageDecision, PageScreening,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::error::AppError;

const MAX_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

/// Request payload for `POST /documents/extract`.
#[derive(Debug, Deserialize)]
pub struct ExtractRequest {
    /// Stable source identifier.
    pub source_id: String,
    /// Input format: `"html"`, `"html_body"`, or `"text"`.
    pub format: String,
    /// UTF-8 snapshot content.
    pub content: String,
    /// When true, withhold normalized text and candidates from storage and response.
    #[serde(default)]
    pub metadata_only: bool,
}

/// Stored or returned acquisition attempt record.
#[derive(Debug, Serialize)]
pub struct AttemptSummary {
    /// Attempt UUID.
    pub id: String,
    /// Source identifier.
    pub source_id: String,
    /// Input format string.
    pub format: String,
    /// Disposition (`accepted`, `skipped`, `uncertain`).
    pub disposition: String,
    /// Optional withholding reason.
    pub reason: Option<String>,
    /// Linked document version ID when accepted and retained.
    pub document_version_id: Option<String>,
    /// Page screening output when available.
    pub page_screening: Option<Value>,
    /// Local HTML metadata when available.
    pub metadata: Option<Value>,
    /// Whether retention was restricted to metadata only.
    pub metadata_only: bool,
}

/// Stored document version summary for list views.
#[derive(Debug, Serialize, FromRow)]
pub struct DocumentSummary {
    /// Document version ID (SHA-256).
    pub id: String,
    /// Source identifier.
    pub source_id: String,
    /// Normalizer version identifier.
    pub normalizer_version: String,
    /// Normalized text digest.
    pub normalized_text_sha256: String,
    /// UTC creation timestamp.
    pub created_at: String,
    /// Number of stored candidates.
    pub candidate_count: i64,
}

/// Stored candidate row returned by the API.
#[derive(Debug, Serialize)]
pub struct StoredCandidate {
    /// Candidate UUID.
    pub id: String,
    /// Extractor version.
    pub extractor_version: String,
    /// Rule identifier.
    pub rule_id: String,
    /// Candidate kind (`date`, `quantity`, `relationship`).
    pub kind: String,
    /// Parsed structured value.
    pub value: Value,
    /// Inclusive UTF-8 start byte.
    pub start_byte: i64,
    /// Exclusive UTF-8 end byte.
    pub end_byte: i64,
    /// Exact evidence quote.
    pub quote: String,
    /// Optional argument spans for relationships.
    pub argument_evidence: Option<Value>,
}

/// Full document detail response.
#[derive(Debug, Serialize)]
pub struct DocumentDetail {
    /// Document version ID.
    pub id: String,
    /// Source identifier.
    pub source_id: String,
    /// Normalizer version.
    pub normalizer_version: String,
    /// Raw payload SHA-256.
    pub payload_sha256: String,
    /// Normalized text SHA-256.
    pub normalized_text_sha256: String,
    /// Normalized text.
    pub normalized_text: String,
    /// UTC creation timestamp.
    pub created_at: String,
    /// Stored candidates in document order.
    pub candidates: Vec<StoredCandidate>,
}

/// Response for `POST /documents/extract`.
#[derive(Debug, Serialize)]
pub struct ExtractResponse {
    /// Recorded attempt disposition.
    pub attempt: AttemptSummary,
    /// Stored document detail when retained.
    pub document: Option<DocumentDetail>,
}

/// Selection item for `POST /reviews`.
#[derive(Debug, Deserialize)]
pub struct ReviewSelection {
    /// Document version ID.
    pub document_version_id: String,
    /// Exact evidence quote.
    pub quote: String,
    /// Optional UTF-8 start byte to disambiguate repeated quotes.
    pub start_byte: Option<usize>,
}

/// Request payload for `POST /reviews`.
#[derive(Debug, Deserialize)]
pub struct ReviewRequest {
    /// Review question.
    pub question: String,
    /// Selected evidence quotes to verify and tally.
    pub selections: Vec<ReviewSelection>,
}

/// Verified item in `POST /reviews`.
#[derive(Debug, Serialize)]
pub struct VerifiedSelection {
    /// Document version ID.
    pub document_version_id: String,
    /// Source ID.
    pub source_id: String,
    /// Verified quote.
    pub quote: String,
    /// Verified start byte.
    pub start_byte: usize,
    /// Verified end byte.
    pub end_byte: usize,
}

/// Response for `POST /reviews`.
#[derive(Debug, Serialize)]
pub struct ReviewResponse {
    /// Review question.
    pub question: String,
    /// Number of verified selections.
    pub total_verified: usize,
    /// Quote frequency across verified selections.
    pub tallies: BTreeMap<String, usize>,
    /// Verified selection details.
    pub verified: Vec<VerifiedSelection>,
    /// Selection failures or ambiguous quotes.
    pub unresolved: Vec<String>,
}

#[derive(FromRow)]
struct DocumentRow {
    id: String,
    source_id: String,
    normalizer_version: String,
    payload_sha256: String,
    normalized_text_sha256: String,
    normalized_text: String,
    created_at: String,
}

#[derive(FromRow)]
struct CandidateRow {
    id: String,
    extractor_version: String,
    rule_id: String,
    kind: String,
    value_json: String,
    start_byte: i64,
    end_byte: i64,
    quote: String,
    argument_evidence_json: Option<String>,
}

/// Extracts and persists a saved UTF-8 snapshot respecting `metadata_only`.
pub async fn extract(
    State(pool): State<SqlitePool>,
    Json(payload): Json<ExtractRequest>,
) -> Result<(StatusCode, Json<ExtractResponse>), AppError> {
    let source_id = payload.source_id.trim();
    if source_id.is_empty() {
        return Err(AppError::BadRequest("source_id is required"));
    }
    if payload.content.len() > MAX_PAYLOAD_BYTES {
        return Err(AppError::BadRequest("content exceeds 2 MiB limit"));
    }
    let format = match payload.format.as_str() {
        "html" => InputFormat::Html,
        "html_body" => InputFormat::HtmlBody,
        "text" => InputFormat::Text,
        _ => return Err(AppError::BadRequest("unsupported format")),
    };

    let inspection = format
        .inspect(source_id, &payload.content)
        .map_err(|_| AppError::BadRequest("invalid input content"))?;
    let screening_json = inspection
        .page_screening
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| AppError::Internal)?;
    let metadata_json = inspection
        .metadata
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| AppError::Internal)?;

    let (disposition, reason) = decide_disposition(
        inspection.page_screening.as_ref(),
        inspection.document.is_some(),
        payload.metadata_only,
    );

    let attempt_id = Uuid::new_v4().to_string();
    let mut stored_doc: Option<DocumentDetail> = None;
    let mut doc_id_for_attempt: Option<String> = None;

    if disposition == "accepted"
        && !payload.metadata_only
        && let Some(doc) = inspection.document.as_ref()
    {
        let extractor = Extractor::new().map_err(|_| AppError::Internal)?;
        let extraction = extractor.extract(doc).map_err(|_| AppError::Internal)?;
        let doc_id = doc.id().to_owned();
        doc_id_for_attempt = Some(doc_id.clone());

        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT OR IGNORE INTO document_versions \
             (id, source_id, normalizer_version, payload_sha256, normalized_text_sha256, normalized_text) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&doc_id)
        .bind(doc.source_id())
        .bind(doc.normalizer_version())
        .bind(doc.payload_sha256())
        .bind(doc.normalized_text_sha256())
        .bind(doc.text())
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM candidates WHERE document_version_id = ?")
            .bind(&doc_id)
            .execute(&mut *tx)
            .await?;

        let mut stored_candidates = Vec::with_capacity(extraction.candidates.len());
        for candidate in &extraction.candidates {
            let cid = Uuid::new_v4().to_string();
            let kind = match &candidate.value {
                CandidateValue::Date { .. } => "date",
                CandidateValue::Quantity { .. } => "quantity",
                CandidateValue::Relationship { .. } => "relationship",
            };
            let value = serde_json::to_value(&candidate.value).map_err(|_| AppError::Internal)?;
            let value_json = serde_json::to_string(&value).map_err(|_| AppError::Internal)?;
            let arg_value = candidate
                .argument_evidence
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| AppError::Internal)?;
            let arg_json = arg_value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| AppError::Internal)?;
            let start_byte =
                i64::try_from(candidate.evidence.start_byte()).map_err(|_| AppError::Internal)?;
            let end_byte =
                i64::try_from(candidate.evidence.end_byte()).map_err(|_| AppError::Internal)?;

            sqlx::query(
                "INSERT INTO candidates \
                 (id, document_version_id, extractor_version, rule_id, kind, value_json, start_byte, end_byte, quote, argument_evidence_json) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&cid)
            .bind(&doc_id)
            .bind(candidate.extractor_version)
            .bind(candidate.rule_id)
            .bind(kind)
            .bind(&value_json)
            .bind(start_byte)
            .bind(end_byte)
            .bind(candidate.evidence.quote())
            .bind(&arg_json)
            .execute(&mut *tx)
            .await?;

            stored_candidates.push(StoredCandidate {
                id: cid,
                extractor_version: candidate.extractor_version.to_owned(),
                rule_id: candidate.rule_id.to_owned(),
                kind: kind.to_owned(),
                value,
                start_byte,
                end_byte,
                quote: candidate.evidence.quote().to_owned(),
                argument_evidence: arg_value,
            });
        }
        tx.commit().await?;

        let created_at: String =
            sqlx::query_scalar("SELECT created_at FROM document_versions WHERE id = ?")
                .bind(&doc_id)
                .fetch_one(&pool)
                .await?;

        stored_doc = Some(DocumentDetail {
            id: doc_id,
            source_id: doc.source_id().to_owned(),
            normalizer_version: doc.normalizer_version().to_owned(),
            payload_sha256: doc.payload_sha256().to_owned(),
            normalized_text_sha256: doc.normalized_text_sha256().to_owned(),
            normalized_text: doc.text().to_owned(),
            created_at,
            candidates: stored_candidates,
        });
    }

    let screening_text = screening_json
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| AppError::Internal)?;
    let metadata_text = metadata_json
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| AppError::Internal)?;

    sqlx::query(
        "INSERT INTO acquisition_attempts \
         (id, source_id, format, disposition, reason, document_version_id, page_screening_json, metadata_json, metadata_only) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&attempt_id)
    .bind(source_id)
    .bind(&payload.format)
    .bind(disposition)
    .bind(reason)
    .bind(&doc_id_for_attempt)
    .bind(&screening_text)
    .bind(&metadata_text)
    .bind(i64::from(payload.metadata_only))
    .execute(&pool)
    .await?;

    let _ = DocumentMetadata::default();
    Ok((
        StatusCode::CREATED,
        Json(ExtractResponse {
            attempt: AttemptSummary {
                id: attempt_id,
                source_id: source_id.to_owned(),
                format: payload.format,
                disposition: disposition.to_owned(),
                reason: reason.map(str::to_owned),
                document_version_id: doc_id_for_attempt,
                page_screening: screening_json,
                metadata: metadata_json,
                metadata_only: payload.metadata_only,
            },
            document: stored_doc,
        }),
    ))
}

/// Lists stored document versions ordered newest first.
pub async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<DocumentSummary>>, AppError> {
    let rows = sqlx::query_as::<_, DocumentSummary>(
        "SELECT d.id, d.source_id, d.normalizer_version, d.normalized_text_sha256, d.created_at, \
         COUNT(c.id) AS candidate_count \
         FROM document_versions d \
         LEFT JOIN candidates c ON c.document_version_id = d.id \
         GROUP BY d.id \
         ORDER BY d.created_at DESC, d.id ASC",
    )
    .fetch_all(&pool)
    .await?;
    Ok(Json(rows))
}

/// Fetches a single stored document version and its candidates.
pub async fn get(
    State(pool): State<SqlitePool>,
    Path(id): Path<String>,
) -> Result<Json<DocumentDetail>, AppError> {
    let row = sqlx::query_as::<_, DocumentRow>(
        "SELECT id, source_id, normalizer_version, payload_sha256, normalized_text_sha256, normalized_text, created_at \
         FROM document_versions WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let candidate_rows = sqlx::query_as::<_, CandidateRow>(
        "SELECT id, extractor_version, rule_id, kind, value_json, start_byte, end_byte, quote, argument_evidence_json \
         FROM candidates WHERE document_version_id = ? ORDER BY start_byte ASC, end_byte ASC",
    )
    .bind(&id)
    .fetch_all(&pool)
    .await?;

    let mut candidates = Vec::with_capacity(candidate_rows.len());
    for cand in candidate_rows {
        let value: Value =
            serde_json::from_str(&cand.value_json).map_err(|_| AppError::Internal)?;
        let argument_evidence = cand
            .argument_evidence_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| AppError::Internal)?;
        candidates.push(StoredCandidate {
            id: cand.id,
            extractor_version: cand.extractor_version,
            rule_id: cand.rule_id,
            kind: cand.kind,
            value,
            start_byte: cand.start_byte,
            end_byte: cand.end_byte,
            quote: cand.quote,
            argument_evidence,
        });
    }

    Ok(Json(DocumentDetail {
        id: row.id,
        source_id: row.source_id,
        normalizer_version: row.normalizer_version,
        payload_sha256: row.payload_sha256,
        normalized_text_sha256: row.normalized_text_sha256,
        normalized_text: row.normalized_text,
        created_at: row.created_at,
        candidates,
    }))
}

/// Verifies selected quotes against stored document versions and tallies unique source support.
pub async fn review(
    State(pool): State<SqlitePool>,
    Json(payload): Json<ReviewRequest>,
) -> Result<Json<ReviewResponse>, AppError> {
    if payload.question.trim().is_empty() {
        return Err(AppError::BadRequest("question is required"));
    }

    let mut verified = Vec::new();
    let mut unresolved = Vec::new();
    let mut tallies: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen_sources = std::collections::HashSet::new();

    for selection in &payload.selections {
        let row = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, source_id, normalizer_version, payload_sha256, normalized_text_sha256, normalized_text, created_at \
             FROM document_versions WHERE id = ?",
        )
        .bind(&selection.document_version_id)
        .fetch_optional(&pool)
        .await?;

        let Some(doc) = row else {
            unresolved.push(format!(
                "{}: document not found",
                selection.document_version_id
            ));
            continue;
        };

        if selection.quote.is_empty() {
            unresolved.push(format!("{}: empty quote", doc.id));
            continue;
        }

        let start = if let Some(offset) = selection.start_byte {
            let end = offset.saturating_add(selection.quote.len());
            if doc.normalized_text.get(offset..end) == Some(selection.quote.as_str()) {
                Some(offset)
            } else {
                None
            }
        } else {
            let matches: Vec<usize> = doc
                .normalized_text
                .match_indices(&selection.quote)
                .map(|(idx, _)| idx)
                .collect();
            if matches.len() == 1 {
                Some(matches[0])
            } else {
                None
            }
        };

        let Some(start_byte) = start else {
            unresolved.push(format!(
                "{}: quote missing or ambiguous without start_byte",
                doc.id
            ));
            continue;
        };

        let end_byte = start_byte + selection.quote.len();
        if seen_sources.insert((doc.source_id.clone(), selection.quote.clone())) {
            *tallies.entry(selection.quote.clone()).or_insert(0) += 1;
        }
        verified.push(VerifiedSelection {
            document_version_id: doc.id,
            source_id: doc.source_id,
            quote: selection.quote.clone(),
            start_byte,
            end_byte,
        });
    }

    Ok(Json(ReviewResponse {
        question: payload.question,
        total_verified: verified.len(),
        tallies,
        verified,
        unresolved,
    }))
}

fn decide_disposition(
    screening: Option<&PageScreening>,
    has_document: bool,
    metadata_only: bool,
) -> (&'static str, Option<&'static str>) {
    if metadata_only {
        return ("skipped", Some("metadata_only"));
    }
    match screening.map(PageScreening::decision) {
        Some(PageDecision::Skipped(reason)) => ("skipped", Some(reason)),
        Some(PageDecision::Uncertain(reason)) => ("uncertain", Some(reason)),
        _ if has_document => ("accepted", None),
        _ => ("skipped", Some("empty_text")),
    }
}
