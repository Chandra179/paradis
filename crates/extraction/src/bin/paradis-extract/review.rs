//! Reviews explicitly selected evidence for one question.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use extraction::{Candidate, CandidateValue, Extraction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    question: String,
    evidence: Vec<Selection>,
}

impl Plan {
    pub(super) fn validate(&self, contains_snapshot: impl Fn(&str) -> bool) -> Result<(), String> {
        if self.question.trim().is_empty() || self.evidence.is_empty() {
            return Err(
                "evidence review needs a question and at least one evidence selection".into(),
            );
        }
        let mut seen = HashSet::new();
        for item in &self.evidence {
            if !contains_snapshot(&item.snapshot_id) || item.quote.trim().is_empty() {
                return Err(format!(
                    "invalid evidence selection for snapshot {:?}",
                    item.snapshot_id
                ));
            }
            if !seen.insert((&item.snapshot_id, &item.quote, item.start_byte)) {
                return Err(format!(
                    "duplicate evidence selection for snapshot {:?}",
                    item.snapshot_id
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    snapshot_id: String,
    quote: String,
    start_byte: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(super) struct Report<'a> {
    review_version: &'static str,
    question: &'a str,
    observations: Vec<Observation<'a>>,
    current_values: Vec<ValueGroup<'a>>,
    conflicting_attestations: Vec<&'a str>,
    active_observations_without_attestation: usize,
}

#[derive(Debug, Serialize)]
struct Observation<'a> {
    selection_index: usize,
    snapshot_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_start_byte: Option<usize>,
    attestation_id: Option<&'a str>,
    historical: bool,
    status: SelectionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate: Option<Candidate>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SelectionStatus {
    Selected,
    SnapshotUnavailable,
    CandidateNotFound,
    AmbiguousCandidate,
    CandidateLimitReached,
}

#[derive(Debug, Serialize)]
struct ValueGroup<'a> {
    value: CandidateValue,
    attestation_count: usize,
    attestation_ids: Vec<&'a str>,
}

pub(super) struct Collector<'a> {
    plan: &'a Plan,
    indices: HashMap<&'a str, Vec<usize>>,
    observations: Vec<Option<Observation<'a>>>,
}

impl<'a> Collector<'a> {
    pub(super) fn new(plan: &'a Plan) -> Self {
        let mut indices: HashMap<_, Vec<_>> = HashMap::new();
        for (index, item) in plan.evidence.iter().enumerate() {
            indices
                .entry(item.snapshot_id.as_str())
                .or_default()
                .push(index);
        }
        Self {
            plan,
            indices,
            observations: (0..plan.evidence.len()).map(|_| None).collect(),
        }
    }

    pub(super) fn record(
        &mut self,
        snapshot_id: &str,
        attestation_id: Option<&'a str>,
        extraction: Option<&Extraction>,
    ) {
        if let Some(indices) = self.indices.get(snapshot_id) {
            for &index in indices {
                let item = &self.plan.evidence[index];
                let (status, candidate) = select(item, extraction);
                self.observations[index] = Some(Observation {
                    selection_index: index + 1,
                    snapshot_id: &item.snapshot_id,
                    requested_start_byte: item.start_byte,
                    attestation_id,
                    historical: false,
                    status,
                    candidate,
                });
            }
        }
    }

    pub(super) fn finish(self, superseded: &HashSet<&str>, withheld: &HashSet<&str>) -> Report<'a> {
        let mut observations: Vec<_> = self.observations.into_iter().flatten().collect();
        let mut by_attestation: BTreeMap<&str, BTreeSet<&CandidateValue>> = BTreeMap::new();
        let mut active_observations_without_attestation = 0;
        for item in &mut observations {
            item.historical = superseded.contains(item.snapshot_id);
            if withheld.contains(item.snapshot_id) {
                item.status = SelectionStatus::SnapshotUnavailable;
                item.candidate = None;
            }
            let Some(candidate) = item.candidate.as_ref().filter(|_| !item.historical) else {
                continue;
            };
            if let Some(id) = item.attestation_id {
                by_attestation
                    .entry(id)
                    .or_default()
                    .insert(&candidate.value);
            } else {
                active_observations_without_attestation += 1;
            }
        }
        let mut conflicting_attestations = Vec::new();
        let mut grouped = BTreeMap::<&CandidateValue, Vec<&str>>::new();
        for (id, values) in by_attestation {
            if values.len() > 1 {
                conflicting_attestations.push(id);
            } else if let Some(value) = values.first() {
                grouped.entry(value).or_default().push(id);
            }
        }
        let current_values = grouped
            .into_iter()
            .map(|(value, attestation_ids)| ValueGroup {
                value: value.clone(),
                attestation_count: attestation_ids.len(),
                attestation_ids,
            })
            .collect();
        Report {
            review_version: "declared-evidence-v1",
            question: &self.plan.question,
            observations,
            current_values,
            conflicting_attestations,
            active_observations_without_attestation,
        }
    }
}

fn select(
    item: &Selection,
    extraction: Option<&Extraction>,
) -> (SelectionStatus, Option<Candidate>) {
    let Some(extraction) = extraction else {
        return (SelectionStatus::SnapshotUnavailable, None);
    };
    let mut matches = extraction.candidates.iter().filter(|candidate| {
        candidate.evidence.quote() == item.quote
            && item
                .start_byte
                .is_none_or(|offset| candidate.evidence.start_byte() == offset)
    });
    match (matches.next(), matches.next()) {
        (Some(_), Some(_)) => (SelectionStatus::AmbiguousCandidate, None),
        (Some(candidate), None) if !extraction.truncated || item.start_byte.is_some() => {
            (SelectionStatus::Selected, Some(candidate.clone()))
        }
        _ if extraction.truncated => (SelectionStatus::CandidateLimitReached, None),
        _ => (SelectionStatus::CandidateNotFound, None),
    }
}
