//! Ordered offline snapshots with declared provenance and retention.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    io::Write,
    path::{Path, PathBuf},
};

use extraction::{Extractor, InputFormat};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{
    MAX_INPUT_BYTES, Record, SavedInput, emit, extract_file,
    provenance::{DeclaredLinks, DocumentHistory, DocumentReference},
    read_snapshot, review,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    snapshots: Vec<Snapshot>,
    review: Option<review::Plan>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    id: String,
    pub(super) source_id: String,
    #[serde(skip_serializing)]
    path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    attestation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supersedes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    copied_from: Option<String>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    captured_at: Option<OffsetDateTime>,
    #[serde(default)]
    retention: Retention,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    retain_until: Option<OffsetDateTime>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Retention {
    #[default]
    NormalizedText,
    MetadataOnly,
}

impl Snapshot {
    pub(super) fn withhold_reason(&self, now: OffsetDateTime) -> Option<&'static str> {
        if self.retain_until.is_some_and(|deadline| now >= deadline) {
            Some("retention_expired")
        } else if matches!(self.retention, Retention::MetadataOnly) {
            Some("metadata_only")
        } else {
            None
        }
    }
}

pub(super) fn run(
    path: &Path,
    extractor: &Extractor,
    output: &mut impl Write,
) -> Result<(), Box<dyn Error>> {
    let source = read_snapshot(path, MAX_INPUT_BYTES)
        .map_err(|reason| format!("read snapshot manifest {}: {reason}", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&source)
        .map_err(|error| format!("parse snapshot manifest {}: {error}", path.display()))?;
    let attestations = manifest.validate()?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut history = DocumentHistory::default();
    let mut documents: HashMap<&str, DocumentReference> = HashMap::new();
    let mut retired_versions = HashSet::new();
    let mut review = manifest.review.as_ref().map(review::Collector::new);

    for (index, snapshot) in manifest.snapshots.iter().enumerate() {
        let supersedes = snapshot
            .supersedes
            .as_deref()
            .and_then(|id| documents.get(id));
        let copied_from = snapshot
            .copied_from
            .as_deref()
            .and_then(|id| documents.get(id));
        let result = extract_file(
            SavedInput {
                path: &base.join(&snapshot.path),
                snapshot: Some(snapshot),
                links: DeclaredLinks {
                    supersedes_version: supersedes.cloned(),
                    copied_from_version: copied_from.cloned(),
                },
            },
            index + 1,
            InputFormat::Html,
            extractor,
            None,
            &mut history,
            output,
        )?;
        if let Some(review) = &mut review {
            review.record(
                &snapshot.id,
                attestations.get(snapshot.id.as_str()).copied().flatten(),
                result.as_ref().map(|accepted| &accepted.extraction),
            );
        }
        if let Some(accepted) = result {
            if let Some(previous) = supersedes
                && !previous.same_version(&accepted.reference)
            {
                retired_versions.insert(previous.version_id().to_owned());
                retired_versions.remove(accepted.reference.version_id());
            }
            documents.insert(snapshot.id.as_str(), accepted.reference);
        }
    }

    if let Some(review) = review {
        let mut superseded = HashSet::<&str>::new();
        let mut withheld = HashSet::<&str>::new();
        let review_time = OffsetDateTime::now_utc();
        for snapshot in &manifest.snapshots {
            if documents
                .get(snapshot.id.as_str())
                .is_some_and(|reference| retired_versions.contains(reference.version_id()))
                || snapshot
                    .copied_from
                    .as_deref()
                    .is_some_and(|id| superseded.contains(id))
            {
                superseded.insert(&snapshot.id);
            }
            if snapshot.withhold_reason(review_time).is_some() {
                withheld.insert(&snapshot.id);
            }
        }
        let report = review.finish(&superseded, &withheld);
        emit(output, Record::EvidenceReview { review: &report })?;
    }
    Ok(())
}

impl Manifest {
    fn validate(&self) -> Result<HashMap<&str, Option<&str>>, String> {
        if self.schema_version != 1 || self.snapshots.is_empty() {
            return Err(
                "snapshot manifest needs schema_version 1 and at least one snapshot".into(),
            );
        }
        let mut seen: HashMap<&str, &Snapshot> = HashMap::new();
        let mut attestations: HashMap<&str, Option<&str>> = HashMap::new();
        for snapshot in &self.snapshots {
            if snapshot.id.trim().is_empty()
                || snapshot.source_id.trim().is_empty()
                || snapshot.path.as_os_str().is_empty()
                || snapshot
                    .attestation_id
                    .as_deref()
                    .is_some_and(|id| id.trim().is_empty())
            {
                return Err(format!(
                    "snapshot {:?} has a blank ID, source, path, or attestation",
                    snapshot.id
                ));
            }
            if seen.contains_key(snapshot.id.as_str()) {
                return Err(format!("duplicate snapshot ID {:?}", snapshot.id));
            }
            let mut attestation = snapshot.attestation_id.as_deref();
            for (label, parent, same_source) in [
                ("supersedes", &snapshot.supersedes, true),
                ("copied_from", &snapshot.copied_from, false),
            ] {
                let Some(parent) = parent.as_deref() else {
                    continue;
                };
                let original = seen.get(parent).ok_or_else(|| {
                    format!(
                        "snapshot {:?} {label} must reference an earlier snapshot",
                        snapshot.id
                    )
                })?;
                if (original.source_id == snapshot.source_id) != same_source {
                    return Err(format!(
                        "snapshot {:?} has inconsistent source IDs for {label}",
                        snapshot.id
                    ));
                }
                let inherited = attestations.get(parent).copied().flatten();
                if let (Some(left), Some(right)) = (attestation, inherited)
                    && left != right
                {
                    return Err(format!(
                        "snapshot {:?} has inconsistent attestations for {label}",
                        snapshot.id
                    ));
                }
                attestation = attestation.or(inherited);
            }
            attestations.insert(snapshot.id.as_str(), attestation);
            seen.insert(snapshot.id.as_str(), snapshot);
        }
        if let Some(review) = &self.review {
            review.validate(|id| seen.contains_key(id))?;
        }
        Ok(attestations)
    }
}
