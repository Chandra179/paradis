//! End-to-end checks for archive identity, correction, and review evidence.

#![allow(clippy::unwrap_used)]

use std::{collections::HashMap, fs, path::Path, process::Command};

use serde_json::{Value, json};

fn run(path: &Path) -> Vec<Value> {
    let result = Command::new(env!("CARGO_BIN_EXE_paradis-extract"))
        .arg("--snapshot-manifest")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn preserves_disagreements_and_counts_reporting_lineages_once() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks/provenance/manifest.json");
    let output = run(&path);
    let attempts: Vec<_> = output
        .iter()
        .filter(|row| row["record_type"] == "attempt")
        .collect();
    let documents: Vec<_> = output
        .iter()
        .filter(|row| row["record_type"] == "document")
        .collect();
    assert_eq!(attempts.len(), 10);
    assert_eq!(documents.len(), 8);
    assert_eq!(attempts[8]["reason"], "retention_expired");
    assert_eq!(attempts[9]["reason"], "metadata_only");
    assert!(attempts[8]["document_version_id"].is_null());
    assert!(attempts[9]["document_version_id"].is_null());
    let serialized = serde_json::to_string(&output).unwrap();
    assert!(!serialized.contains("88 kg"));
    assert!(!serialized.contains("99 kg"));
    let original = &documents[0]["document"];
    assert_eq!(documents[1]["document"]["id"], original["id"]);
    assert_eq!(
        documents[2]["exact_text_duplicate_of"]["document_version_id"],
        original["id"]
    );
    assert_eq!(
        documents[2]["copied_from_version"]["document_version_id"],
        original["id"]
    );
    assert_eq!(
        documents[3]["copied_from_version"]["document_version_id"],
        documents[2]["document"]["id"]
    );
    assert!(documents[3].get("exact_text_duplicate_of").is_none());
    assert_eq!(documents[6]["document"]["source_id"], original["source_id"]);
    assert_ne!(documents[6]["document"]["id"], original["id"]);
    assert_eq!(
        documents[6]["supersedes_version"]["document_version_id"],
        original["id"]
    );
    assert_eq!(
        documents[6]["previous_observed_version"]["attempt_index"],
        2
    );

    let review = output.last().unwrap();
    assert_eq!(review["record_type"], "evidence_review");
    assert_eq!(
        review["current_values"],
        json!([
            {"value":{"kind":"quantity","value":"5","unit":"kg"},"attestation_count":2,"attestation_ids":["audit","gallery"]},
            {"value":{"kind":"quantity","value":"7","unit":"kg"},"attestation_count":1,"attestation_ids":["survey"]}
        ])
    );
    assert_eq!(review["conflicting_attestations"], json!([]));
    assert_eq!(review["active_observations_without_attestation"], 1);
    let versions: HashMap<_, _> = documents
        .iter()
        .map(|row| {
            let document = &row["document"];
            (
                document["id"].as_str().unwrap(),
                document["normalized_text"].as_str().unwrap(),
            )
        })
        .collect();
    let observations = review["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 10);
    for (index, observation) in observations.iter().enumerate() {
        assert_eq!(observation["historical"], index < 4);
        if index >= 8 {
            assert_eq!(observation["status"], "snapshot_unavailable");
            assert!(observation.get("candidate").is_none());
            continue;
        }
        let evidence = &observation["candidate"]["evidence"];
        let version = evidence["document_version_id"].as_str().unwrap();
        let start = usize::try_from(evidence["start_byte"].as_u64().unwrap()).unwrap();
        let end = usize::try_from(evidence["end_byte"].as_u64().unwrap()).unwrap();
        assert_eq!(observation["status"], "selected");
        assert_eq!(
            versions[version].get(start..end),
            evidence["quote"].as_str()
        );
    }
    assert_eq!(run(&path).last(), Some(review));
}

#[test]
fn leaves_ambiguous_evidence_and_disagreement_within_one_lineage_unresolved() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("manifest.json");
    let first = "Record A reads 5 kg and 5 kg.";
    fs::write(directory.path().join("a.txt"), first).unwrap();
    fs::write(directory.path().join("b.txt"), "Record B reads 7 kg.").unwrap();
    fs::write(
        &manifest,
        json!({
            "schema_version":1,
            "snapshots":[
               {"id":"a","path":"a.txt","source_id":"a","attestation_id":"release"},
               {"id":"b","path":"b.txt","source_id":"b","copied_from":"a"}
            ],
            "review":{"question":"What does the release report?","evidence":[
                {"snapshot_id":"a","quote":"5 kg"},
                {"snapshot_id":"a","quote":"5 kg","start_byte":first.find("5 kg").unwrap()},
                {"snapshot_id":"b","quote":"7 kg"},
                {"snapshot_id":"b","quote":"11 kg"}
            ]}
        })
        .to_string(),
    )
    .unwrap();
    let output = run(&manifest);
    let review = output.last().unwrap();
    assert_eq!(review["current_values"], json!([]));
    assert_eq!(review["conflicting_attestations"], json!(["release"]));
    let statuses: Vec<_> = review["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|observation| observation["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        [
            "ambiguous_candidate",
            "selected",
            "selected",
            "candidate_not_found"
        ]
    );
}

#[test]
fn rejects_inconsistent_source_links_before_writing_records() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("manifest.json");
    fs::write(
        &manifest,
        json!({
            "schema_version":1,
            "snapshots":[
               {"id":"a","path":"a.txt","source_id":"publisher-a"},
               {"id":"b","path":"b.txt","source_id":"publisher-b","supersedes":"a"}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_paradis-extract"))
        .arg("--snapshot-manifest")
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("inconsistent source IDs for supersedes")
    );
}
