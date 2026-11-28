//! Tracks exact copies and observed source versions within one extraction batch.

use std::collections::{HashMap, hash_map::Entry};

use extraction::DocumentVersion;
use serde::Serialize;

#[derive(Debug, Default)]
pub(super) struct DocumentHistory {
    first_by_text: HashMap<String, DocumentReference>,
    latest_by_source: HashMap<String, DocumentReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DocumentReference {
    attempt_index: usize,
    document_version_id: String,
}

impl DocumentReference {
    pub(super) fn new(attempt_index: usize, document_version_id: &str) -> Self {
        Self {
            attempt_index,
            document_version_id: document_version_id.to_owned(),
        }
    }

    pub(super) fn matches(&self, document_version_id: &str) -> bool {
        self.document_version_id == document_version_id
    }

    pub(super) fn same_version(&self, other: &Self) -> bool {
        self.matches(&other.document_version_id)
    }

    pub(super) fn version_id(&self) -> &str {
        &self.document_version_id
    }
}

#[derive(Debug, Default, Serialize)]
pub(super) struct DeclaredLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) supersedes_version: Option<DocumentReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) copied_from_version: Option<DocumentReference>,
}

#[derive(Debug, Serialize)]
pub(super) struct Provenance {
    #[serde(skip_serializing_if = "Option::is_none")]
    exact_text_duplicate_of: Option<DocumentReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_observed_version: Option<DocumentReference>,
    #[serde(flatten)]
    pub(super) declared: DeclaredLinks,
}

impl DocumentHistory {
    /// Records a document while preserving both copy and version links.
    pub(super) fn observe(
        &mut self,
        attempt_index: usize,
        document: &DocumentVersion,
    ) -> Provenance {
        let current = DocumentReference::new(attempt_index, document.id());
        let exact_text_duplicate_of = match self
            .first_by_text
            .entry(document.normalized_text_sha256().to_owned())
        {
            Entry::Occupied(entry) => Some(entry.get().clone()),
            Entry::Vacant(entry) => {
                entry.insert(current.clone());
                None
            }
        };
        let previous_observed_version = self
            .latest_by_source
            .insert(document.source_id().to_owned(), current)
            .filter(|previous| previous.document_version_id != document.id());
        Provenance {
            exact_text_duplicate_of,
            previous_observed_version,
            declared: DeclaredLinks::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_revisions_and_keeps_the_first_copy_reference() {
        let first =
            DocumentVersion::from_normalized("source", b"3 kg", "plain-v1", "3 kg".into()).unwrap();
        let corrected =
            DocumentVersion::from_normalized("source", b"5 kg", "plain-v1", "5 kg".into()).unwrap();
        let copy =
            DocumentVersion::from_normalized("copy", b"3 kg", "plain-v1", "3 kg".into()).unwrap();
        let mut history = DocumentHistory::default();
        assert!(history.observe(1, &first).exact_text_duplicate_of.is_none());
        assert_eq!(
            history
                .observe(2, &corrected)
                .previous_observed_version
                .unwrap()
                .document_version_id,
            first.id()
        );
        let replay = history.observe(3, &corrected);
        assert!(replay.previous_observed_version.is_none());
        assert_eq!(replay.exact_text_duplicate_of.unwrap().attempt_index, 2);
        let syndicated = history.observe(4, &copy);
        assert!(syndicated.previous_observed_version.is_none());
        assert_eq!(syndicated.exact_text_duplicate_of.unwrap().attempt_index, 1);
    }
}
