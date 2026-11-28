//! Evidence-backed information extraction from saved documents.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::{error::Error, fmt, ops::Range};

use serde::Serialize;
use sha2::{Digest, Sha256};

mod candidates;
mod dependencies;
mod metadata;
mod normalize;
mod screening;
pub mod sitemap;
pub mod source_registry;

pub use candidates::{ArgumentEvidence, Candidate, CandidateValue, Extraction, Extractor};
pub use dependencies::{DependencyError, DependencyParse};
pub use metadata::{DocumentMetadata, MetadataDisagreement};
pub use normalize::{InputFormat, SnapshotInspection};
pub use screening::{PageDecision, PageScreening};

/// A version of a source document and the exact text its evidence cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentVersion {
    id: String,
    source_id: String,
    payload_sha256: String,
    normalized_text_sha256: String,
    normalizer_version: String,
    normalized_text: String,
}

impl DocumentVersion {
    /// Records a normalized document and hashes the bytes supplied to its parser.
    ///
    /// Supply a stable source identifier and a version for the normalization
    /// procedure. Identical inputs reproduce the same document version ID.
    ///
    /// # Errors
    ///
    /// Returns an error if either identifier is blank.
    pub fn from_normalized(
        source_id: &str,
        parser_input: &[u8],
        normalizer_version: &str,
        normalized_text: String,
    ) -> Result<Self, EvidenceError> {
        if source_id.trim().is_empty() {
            return Err(EvidenceError::MissingSourceId);
        }
        if normalizer_version.trim().is_empty() {
            return Err(EvidenceError::MissingNormalizerVersion);
        }

        let payload_sha256 = sha256_hex(parser_input);
        let normalized_text_sha256 = sha256_hex(normalized_text.as_bytes());
        let mut version = Sha256::new_with_prefix(b"paradis:document-version:1");
        // Length prefixes distinguish different splits between adjacent fields.
        for field in [
            source_id,
            normalizer_version,
            &payload_sha256,
            &normalized_text_sha256,
        ] {
            version.update((field.len() as u64).to_be_bytes());
            version.update(field.as_bytes());
        }

        Ok(Self {
            id: hex_digest(&version.finalize()),
            source_id: source_id.to_owned(),
            payload_sha256,
            normalized_text_sha256,
            normalizer_version: normalizer_version.to_owned(),
            normalized_text,
        })
    }

    /// Returns the stable document version ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the canonical identifier for the underlying source.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the SHA-256 digest of the raw bytes supplied to parsing.
    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }

    /// Returns the version identifier of the normalizer that produced this text.
    pub fn normalizer_version(&self) -> &str {
        &self.normalizer_version
    }

    /// Returns the digest of the exact text used by the extractor.
    pub fn normalized_text_sha256(&self) -> &str {
        &self.normalized_text_sha256
    }

    /// Returns the exact normalized text used by the extractor.
    pub fn text(&self) -> &str {
        &self.normalized_text
    }

    /// Captures a quote at a half-open range of UTF-8 byte offsets.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, reversed, out-of-bounds, or split-character span.
    pub fn evidence(&self, bytes: Range<usize>) -> Result<EvidenceSpan, EvidenceError> {
        let quote = self
            .text()
            .get(bytes.clone())
            .filter(|text| !text.is_empty())
            .ok_or(EvidenceError::InvalidSpan)?;

        Ok(EvidenceSpan {
            document_version_id: self.id.clone(),
            start_byte: bytes.start,
            end_byte: bytes.end,
            quote: quote.to_owned(),
        })
    }

    /// Checks that the cited version and byte range resolve to the quoted text.
    pub fn validates(&self, evidence: &EvidenceSpan) -> bool {
        evidence.document_version_id == self.id
            && !evidence.quote.is_empty()
            && self.text().get(evidence.start_byte..evidence.end_byte) == Some(&evidence.quote)
    }
}

/// A quote anchored by a document version and half-open UTF-8 byte offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceSpan {
    document_version_id: String,
    start_byte: usize,
    end_byte: usize,
    quote: String,
}

impl EvidenceSpan {
    /// Returns the cited document version ID.
    pub fn document_version_id(&self) -> &str {
        &self.document_version_id
    }

    /// Returns the starting byte offset in the cited normalized text.
    pub fn start_byte(&self) -> usize {
        self.start_byte
    }

    /// Returns the exclusive ending byte offset in the cited normalized text.
    pub fn end_byte(&self) -> usize {
        self.end_byte
    }

    /// Returns the exact excerpt from the cited document version.
    pub fn quote(&self) -> &str {
        &self.quote
    }
}

/// Invalid provenance or byte offsets for an evidence record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceError {
    /// The input HTML could not be parsed.
    InvalidHtml,
    /// A stable source ID is required.
    MissingSourceId,
    /// A normalization version is required.
    MissingNormalizerVersion,
    /// The byte range must contain nonempty text on UTF-8 boundaries.
    InvalidSpan,
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::InvalidHtml => "input HTML could not be parsed",
            Self::MissingSourceId => "source_id is required",
            Self::MissingNormalizerVersion => "normalizer_version is required",
            Self::InvalidSpan => "evidence must be a nonempty range on UTF-8 character boundaries",
        };
        f.write_str(description)
    }
}

impl Error for EvidenceError {}

/// Returns a lowercase hexadecimal SHA-256 digest of the supplied bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        hex.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(HEX_DIGITS[usize::from(byte & 15)]));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(source: &str, payload: &[u8], normalizer: &str, text: &str) -> DocumentVersion {
        DocumentVersion::from_normalized(source, payload, normalizer, text.to_owned()).unwrap()
    }

    #[test]
    fn identical_inputs_reproduce_the_document_and_hashes() {
        let first = document("source", b"abc", "plain-v1", "abc");
        let second = document("source", b"abc", "plain-v1", "abc");
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

        assert_eq!(first, second);
        assert_eq!(first.payload_sha256, expected);
        assert_eq!(first.normalized_text_sha256, expected);
    }

    #[test]
    fn different_sources_representations_and_normalizers_have_distinct_versions() {
        let original = document("source-a", b"abc", "plain-v1", "abc");
        for alternative in [
            document("source-b", b"abc", "plain-v1", "abc"),
            document("source-a", b"<p>abc</p>", "plain-v1", "abc"),
            document("source-a", b"abc", "plain-v1", "ABC"),
            document("source-a", b"abc", "plain-v2", "abc"),
        ] {
            assert_ne!(original.id(), alternative.id());
        }
    }

    #[test]
    fn evidence_resolves_utf8_offsets_and_rejects_invalid_ranges() {
        let version = document("source", "Café".as_bytes(), "plain-v1", "Café");
        let span = version.evidence(3..5).unwrap();

        assert_eq!(span.quote, "é");
        assert!(version.validates(&span));
        assert_eq!(version.evidence(3..4), Err(EvidenceError::InvalidSpan));
        assert_eq!(version.evidence(0..6), Err(EvidenceError::InvalidSpan));
        assert_eq!(version.evidence(1..1), Err(EvidenceError::InvalidSpan));
    }

    #[test]
    fn evidence_rejects_a_different_version_or_an_altered_quote() {
        let original = document("source", b"abc", "plain-v1", "abc");
        let revised = document("source", b"abcd", "plain-v1", "abcd");
        let mut evidence = original.evidence(0..3).unwrap();

        assert!(!revised.validates(&evidence));
        evidence.quote = "abd".to_owned();
        assert!(!original.validates(&evidence));
    }

    #[test]
    fn inspect_and_extract_legible_metadata_and_argument_spans() {
        let html = r#"<!doctype html>
<html>
<head>
  <meta property="article:published_time" content="2026-11-26">
  <script type="application/ld+json">
  {
    "@context": "https://schema.org",
    "@type": "NewsArticle",
    "headline": "Gallery Expansion",
    "datePublished": "2026-11-27",
    "author": {"@type": "Person", "name": "Ada Lovelace"}
  }
  </script>
</head>
<body>
  <div class="sidebar">Navigation Archive 2018-01-01</div>
  <div class="story">
    <h1>Gallery Expansion</h1>
    <p>Northwind Labs announced the new gallery wing in November 2026. The expansion covers 420 square meters of exhibition space and includes a conservation workshop for visiting researchers and archivists.</p>
    <p>Northwind Labs founded the archive in 2019. The new wing supports 30 kg per shelf.</p>
  </div>
</body>
</html>"#;
        let inspection = InputFormat::Html.inspect("gallery-story", html).unwrap();
        let meta = inspection.metadata.unwrap();
        assert_eq!(meta.title.as_deref(), Some("Gallery Expansion"));
        assert_eq!(meta.published_at.as_deref(), Some("2026-11-27"));
        assert_eq!(meta.disagreements.len(), 1);

        let doc = inspection.document.unwrap();
        assert_eq!(doc.normalizer_version(), "legible-html-v1");
        assert!(!doc.text().contains("2018-01-01"));

        let extraction = Extractor::new().unwrap().extract(&doc).unwrap();
        let rels: Vec<_> = extraction
            .candidates
            .iter()
            .filter(|c| matches!(c.value, CandidateValue::Relationship { .. }))
            .collect();
        assert!(!rels.is_empty());
        for rel in rels {
            let args = rel.argument_evidence.as_ref().unwrap();
            assert!(doc.validates(&args.subject));
            assert!(doc.validates(&args.predicate));
            assert!(doc.validates(&args.object));
        }
    }
}
