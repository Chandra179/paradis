//! End-to-end checks for JSON Lines output and source evidence.

#![allow(clippy::unwrap_used)]

use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};

fn run(paths: &[&Path]) -> Vec<Value> {
    run_with(&[], paths)
}

fn run_with(options: &[&str], paths: &[&Path]) -> Vec<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_paradis-extract"))
        .args(options)
        .args(paths)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn does_not_emit_a_fraction_fragment_or_unscaled_money() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("quantities.txt");
    fs::write(
        &source,
        "Add 1/2 kg or 1 / 2 kg. The price was $1.2 million. A spare costs $2.50.",
    )
    .unwrap();
    let output = run(&[&source]);
    let quantities: Vec<_> = output
        .iter()
        .filter(|r| r["candidate"]["value"]["kind"] == "quantity")
        .collect();
    assert_eq!(quantities.len(), 2);
    assert_eq!(quantities[0]["candidate"]["value"]["value"], "1200000");
    assert_eq!(
        quantities[0]["candidate"]["evidence"]["quote"],
        "$1.2 million"
    );
    assert_eq!(quantities[1]["candidate"]["value"]["value"], "2.50");
}

#[test]
fn abstains_when_a_second_clause_would_become_part_of_the_object() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("relations.txt");
    fs::write(&source, "The museum contains paintings and documents the restoration. The café contains a reading room and a gallery.").unwrap();
    let output = run(&[&source]);
    let relationships: Vec<_> = output
        .iter()
        .filter(|r| r["candidate"]["value"]["kind"] == "relationship")
        .collect();
    assert_eq!(relationships.len(), 1);
    assert_eq!(
        relationships[0]["candidate"]["value"]["object"],
        "a reading room and a gallery"
    );
}

#[test]
fn dependency_rules_resolve_coordination_and_anchor_unicode_arguments() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("relations.txt");
    let annotations = directory.path().join("dependencies.json");
    let text = "The café contains paintings and sculptures.\nThe café contains paintings and documents the restoration.\nThe café may contain paintings.";
    fs::write(&source, text).unwrap();
    let baseline = run(&[&source]);
    let hash = &baseline[1]["document"]["normalized_text_sha256"];
    let mut offset = 0;
    let mut sentence = |tokens: &[(&str, Option<usize>, &str, &str)]| {
        tokens
            .iter()
            .map(|&(word, head, pos, relation)| {
                offset += text[offset..].len() - text[offset..].trim_start().len();
                let start = offset;
                offset += word.len();
                assert_eq!(&text[start..offset], word);
                json!({
                    "start_byte": start, "end_byte": offset,
                    "head": head, "pos": pos, "relation": relation
                })
            })
            .collect::<Vec<_>>()
    };
    let mut dependencies = json!({
        "parser": "test-parser/1",
        "normalized_text_sha256": hash,
        "sentences": [
            sentence(&[
                ("The", Some(1), "DET", "det"),
                ("café", Some(2), "NOUN", "nsubj"),
                ("contains", None, "VERB", "ROOT"),
                ("paintings", Some(2), "NOUN", "dobj"),
                ("and", Some(3), "CCONJ", "cc"),
                ("sculptures", Some(3), "NOUN", "conj"),
                (".", Some(2), "PUNCT", "punct"),
            ]),
            sentence(&[
                ("The", Some(1), "DET", "det"),
                ("café", Some(2), "NOUN", "nsubj"),
                ("contains", None, "VERB", "ROOT"),
                ("paintings", Some(2), "NOUN", "dobj"),
                ("and", Some(2), "CCONJ", "cc"),
                ("documents", Some(2), "VERB", "conj"),
                ("the", Some(7), "DET", "det"),
                ("restoration", Some(5), "NOUN", "dobj"),
                (".", Some(2), "PUNCT", "punct"),
            ]),
            sentence(&[
                ("The", Some(1), "DET", "det"),
                ("café", Some(3), "NOUN", "nsubj"),
                ("may", Some(3), "AUX", "aux"),
                ("contain", None, "VERB", "ROOT"),
                ("paintings", Some(3), "NOUN", "dobj"),
                (".", Some(3), "PUNCT", "punct"),
            ]),
        ]
    });
    fs::write(&annotations, dependencies.to_string()).unwrap();
    let options = ["--dependencies", annotations.to_str().unwrap(), "--"];
    let output = run_with(&options, &[&source]);
    assert_eq!(output[1]["document"]["id"], baseline[1]["document"]["id"]);
    assert_eq!(output[1]["candidate_count"], 2);
    for (index, expected) in ["paintings and sculptures", "paintings"].iter().enumerate() {
        let candidate = &output[index + 2]["candidate"];
        assert_eq!(candidate["value"]["object"], *expected);
        assert_eq!(candidate["value"]["subject"], "The café");
        assert_eq!(candidate["extractor_version"], "dependency-v1");
        assert_eq!(candidate["syntax_parser"], "test-parser/1");
        let evidence = &candidate["evidence"];
        assert_eq!(evidence["document_version_id"], output[1]["document"]["id"]);
        assert_eq!(
            evidence["quote"],
            text.lines().nth(index).unwrap().trim_end_matches('.')
        );
        let start = usize::try_from(evidence["start_byte"].as_u64().unwrap()).unwrap();
        let end = usize::try_from(evidence["end_byte"].as_u64().unwrap()).unwrap();
        assert_eq!(&text[start..end], evidence["quote"]);
    }

    let rejected = |expected| {
        let result = Command::new(env!("CARGO_BIN_EXE_paradis-extract"))
            .args(options)
            .arg(&source)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("\"record_type\":\"candidate\""));
    };
    dependencies["normalized_text_sha256"] = json!("stale");
    fs::write(&annotations, dependencies.to_string()).unwrap();
    rejected("does not match the normalized text hash");

    dependencies["normalized_text_sha256"] = hash.clone();
    dependencies["sentences"][0][4]["head"] = json!(4);
    fs::write(&annotations, dependencies.to_string()).unwrap();
    rejected("does not form a single rooted tree");
}

#[test]
fn html_body_control_preserves_boilerplate_with_its_own_version() {
    let sources = tempfile::tempdir().unwrap();
    let html = sources.path().join("article.html");
    fs::write(
        &html,
        "<nav>Archive 2019-08-12</nav><main><p>Inspection 2026-11-27.</p></main>",
    )
    .unwrap();

    let pruned = run(&[&html]);
    let body = run_with(&["--html-body", "--"], &[&html]);
    let pruned_doc = &pruned[1]["document"];
    let body_doc = &body[1]["document"];
    assert_eq!(pruned_doc["normalized_text"], "Inspection 2026-11-27.");
    assert!(
        body_doc["normalized_text"]
            .as_str()
            .unwrap()
            .contains("Archive 2019-08-12")
    );
    assert_eq!(pruned_doc["payload_sha256"], body_doc["payload_sha256"]);
    assert_ne!(pruned_doc["id"], body_doc["id"]);
    assert_eq!(body_doc["normalizer_version"], "body-html-v1");
    assert_eq!(body[2]["candidate"]["value"]["iso_date"], "2019-08-12");
    assert_eq!(
        body[2]["candidate"]["evidence"]["document_version_id"],
        body_doc["id"]
    );
}

#[test]
fn links_exact_copies_without_merging_conflicting_sources_or_evidence() {
    let sources = tempfile::tempdir().unwrap();
    let original = sources.path().join("original.txt");
    let copy = sources.path().join("copy.html");
    let disagreement = sources.path().join("disagreement.txt");
    fs::write(&original, "The cabinet contains 3 kg of supplies.").unwrap();
    fs::write(
        &copy,
        "<article><p>The cabinet contains 3 kg of supplies.</p></article>",
    )
    .unwrap();
    fs::write(&disagreement, "The cabinet contains 5 kg of supplies.").unwrap();

    let output = run(&[&original, &copy, &disagreement, &original]);
    let documents: Vec<_> = output
        .iter()
        .filter(|r| r["record_type"] == "document")
        .collect();
    assert_eq!(documents.len(), 4);
    let first = &documents[0]["document"];
    let copied = &documents[1]["document"];
    assert_ne!(first["source_id"], copied["source_id"]);
    assert_ne!(first["payload_sha256"], copied["payload_sha256"]);
    assert_eq!(
        first["normalized_text_sha256"],
        copied["normalized_text_sha256"]
    );
    assert_eq!(
        documents[1]["exact_text_duplicate_of"]["document_version_id"],
        first["id"]
    );
    assert_eq!(documents[3]["exact_text_duplicate_of"]["attempt_index"], 1);
    assert!(documents[2].get("exact_text_duplicate_of").is_none());
    assert!(
        documents
            .iter()
            .all(|row| row.get("previous_observed_version").is_none())
    );

    for (attempt, expected) in [(1, "3"), (2, "3"), (3, "5"), (4, "3")] {
        let quantity = output
            .iter()
            .find(|r| {
                r["attempt_index"] == attempt && r["candidate"]["value"]["kind"] == "quantity"
            })
            .unwrap();
        let candidate = &quantity["candidate"];
        assert_eq!(candidate["value"]["value"], expected);
        assert_eq!(
            candidate["evidence"]["document_version_id"],
            documents[attempt - 1]["document"]["id"]
        );
    }
}

#[test]
fn extracts_valid_named_ordinal_dates_and_rejects_invalid_suffixes() {
    let sources = tempfile::tempdir().unwrap();
    let text = sources.path().join("dates.txt");
    fs::write(
        &text,
        "Meet on 22nd September 2026 or March 11th, 2025. Invalid: March 11st, 2025 and 31st April 2026.",
    ).unwrap();
    let output = run(&[&text]);
    let rows: Vec<_> = output
        .iter()
        .filter(|row| row["record_type"] == "candidate")
        .collect();
    assert_eq!(rows.len(), 2);
    for (row, (date, quote)) in rows.into_iter().zip([
        ("2026-09-22", "22nd September 2026"),
        ("2025-03-11", "March 11th, 2025"),
    ]) {
        let candidate = &row["candidate"];
        assert_eq!(candidate["rule_id"], "named_ordinal_date");
        assert_eq!(candidate["value"]["iso_date"], date);
        assert_eq!(candidate["evidence"]["quote"], quote);
    }
}

#[test]
fn emits_repeatable_candidates_and_utf8_evidence_for_html_and_text() {
    let sources = tempfile::tempdir().unwrap();
    let html = sources.path().join("article.html");
    let text = sources.path().join("notes.txt");
    fs::write(
        &html,
        r#"<nav>Archive contains 300 pages.</nav>
      <main><article><h1>Café inventory</h1>
      <p>The café contains a reading room and a <em>gallery</em>.</p>
      <p>Recorded <time>29 February 2024</time> with a budget of $1,200.50.</p>
      <p hidden>The hall contains 50 boxes.</p><script>Window contains 200 records.</script>
      <footer>25 percent</footer></article></main>"#,
    )
    .unwrap();
    fs::write(
        &text,
        "The gallery is located in the west wing.\r\nUses 30 kWh.\r\nPublished 2026-11-27.\n",
    )
    .unwrap();

    let output = run(&[&html, &text]);
    let semantic = |rows: Vec<Value>| {
        rows.into_iter()
            .filter(|row| row["record_type"] != "attempt")
            .collect::<Vec<_>>()
    };
    assert_eq!(semantic(output.clone()), semantic(run(&[&html, &text])));
    let mut rows = output.iter();
    for expected in [
        [
            (
                "relationship",
                "The café contains a reading room and a gallery",
            ),
            ("date", "29 February 2024"),
            ("quantity", "$1,200.50"),
        ],
        [
            ("relationship", "The gallery is located in the west wing"),
            ("quantity", "30 kWh"),
            ("date", "2026-11-27"),
        ],
    ] {
        let attempt = rows.next().unwrap();
        let document_row = rows.next().unwrap();
        let document = &document_row["document"];
        let normalized = document["normalized_text"].as_str().unwrap();
        assert_eq!(attempt["disposition"], "accepted");
        assert_eq!(attempt["document_version_id"], document["id"]);
        assert_eq!(document_row["candidate_count"], 3);
        for excluded in ["Archive", "Window", "The hall", "25 percent", "\r"] {
            assert!(!normalized.contains(excluded));
        }
        for (kind, quote) in expected {
            let candidate = &rows.next().unwrap()["candidate"];
            let evidence = &candidate["evidence"];
            let start = evidence["start_byte"].as_u64().unwrap() as usize;
            let end = evidence["end_byte"].as_u64().unwrap() as usize;
            assert_eq!(candidate["value"]["kind"], kind);
            assert_eq!(evidence["document_version_id"], document["id"]);
            assert_eq!(evidence["quote"], quote);
            assert_eq!(normalized.get(start..end), Some(quote));
        }
    }
    assert_eq!(output[4]["candidate"]["value"]["value"], "1200.50");
    assert_eq!(output[4]["candidate"]["value"]["unit"], "$");
    assert!(rows.next().is_none());
}

#[test]
fn logs_skipped_inputs_and_leaves_ambiguous_mentions_unresolved() {
    let sources = tempfile::tempdir().unwrap();
    let missing = sources.path().join("missing.html");
    let empty = sources.path().join("empty.txt");
    let invalid = sources.path().join("invalid.txt");
    let unsupported = sources.path().join("data.json");
    let oversized = sources.path().join("oversized.txt");
    let text = sources.path().join("notes.txt");
    fs::write(&empty, " \t\n").unwrap();
    fs::write(&invalid, [0xff]).unwrap();
    fs::File::create(&oversized)
        .unwrap()
        .set_len(2 * 1024 * 1024 + 1)
        .unwrap();
    fs::write(&text, "The gallery may contain a library. The report says the gallery contains a library.\n+         Invalid dates: 2023-02-29 or 31 NOVEMBER 2026.\n+         Partial quantity examples: X30km, 10–20 kg and 50 km/h.\n+         Valid date: NOVEMBER 27, 2026.").unwrap();

    let output = run(&[&missing, &empty, &invalid, &unsupported, &oversized, &text]);
    let mut rows = output.iter();
    for reason in [
        "unreadable_input",
        "empty_text",
        "invalid_utf8",
        "unsupported_format",
        "input_too_large",
    ] {
        let row = rows.next().unwrap();
        assert_eq!(row["record_type"], "attempt");
        assert_eq!(row["disposition"], "skipped");
        assert_eq!(row["reason"], reason);
    }
    assert_eq!(rows.next().unwrap()["disposition"], "accepted");
    assert_eq!(rows.next().unwrap()["candidate_count"], 1);
    assert_eq!(
        rows.next().unwrap()["candidate"]["value"]["iso_date"],
        "2026-11-27"
    );
    assert!(rows.next().is_none());
}

#[test]
fn screens_saved_pages_and_preserves_nearby_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let fixtures: Value =
        serde_json::from_str(include_str!("../benchmarks/page-screening.json")).unwrap();
    let fixtures = fixtures["fixtures"].as_array().unwrap();
    let ids = [
        "explicit-soft-404",
        "structured-consent-only",
        "empty-js-mount",
        "missing-body",
        "whitespace-input",
        "error-documentation",
        "article-with-consent-banner",
    ];
    let selected: Vec<_> = ids
        .iter()
        .map(|id| {
            let fixture = fixtures
                .iter()
                .find(|fixture| fixture["id"] == *id)
                .unwrap();
            let path = directory.path().join(format!("{id}.html"));
            fs::write(&path, fixture["snapshot_text"].as_str().unwrap()).unwrap();
            (fixture, path)
        })
        .collect();
    let paths: Vec<_> = selected.iter().map(|(_, path)| path.as_path()).collect();
    let output = run(&paths);
    for (index, (expected, _)) in selected.iter().enumerate() {
        let rows: Vec<_> = output
            .iter()
            .filter(|record| record["attempt_index"] == index + 1)
            .collect();
        let attempt = rows[0];
        assert_eq!(attempt["disposition"], expected["expected_disposition"]);
        assert_eq!(attempt["reason"], expected["expected_reason"]);
        assert_eq!(attempt["page_screening"]["version"], "html-screen-v1");
        if attempt["disposition"] == "accepted" {
            assert!(
                rows[1]["document"]["normalized_text"]
                    .as_str()
                    .unwrap()
                    .contains(expected["reference_text"].as_str().unwrap())
            );
        } else {
            assert_eq!(rows.len(), 1);
            assert!(attempt["document_version_id"].is_null());
            assert!(
                !attempt["page_screening"]["signals"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
    let raw = run_with(&["--html-body", "--"], &paths[..1]);
    assert_eq!(raw[0]["disposition"], "accepted");
    assert!(raw[0].get("page_screening").is_none());
    assert!(
        raw[1]["document"]["normalized_text"]
            .as_str()
            .unwrap()
            .contains("find the page")
    );
}

#[test]
fn recovers_evidence_when_the_selected_region_is_empty() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("notice.html");
    fs::write(
        &source,
        "<nav>Archive 2019-08-12</nav><main></main><article><p>Reopens 2026-11-27.</p></article>",
    )
    .unwrap();
    let output = run(&[&source]);
    assert_eq!(output[0]["disposition"], "accepted");
    assert_eq!(
        output[0]["page_screening"]["signals"],
        json!(["empty_primary_region", "body_fallback_used"])
    );
    let document = &output[1]["document"];
    assert_eq!(document["normalized_text"], "Reopens 2026-11-27.");
    assert_eq!(
        document["normalizer_version"],
        "semantic-html-body-fallback-v1"
    );
    assert_eq!(output[1]["candidate_count"], 1);
    let candidate = &output[2]["candidate"];
    assert_eq!(candidate["value"]["iso_date"], "2026-11-27");
    assert_eq!(candidate["evidence"]["document_version_id"], document["id"]);
    assert_eq!(candidate["evidence"]["quote"], "2026-11-27");
}

#[test]
fn recovers_marked_content_before_falling_back_to_the_body() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("marked.html");
    for (marker, expected_signal) in [
        ("itemprop=\"articleBody\"", "article_body_marker_used"),
        ("role=\"main\"", "main_role_marker_used"),
    ] {
        fs::write(
            &source,
            format!("<main></main><div>Related notice from 2020-05-02</div><div {marker}><p>The café reopens 2026-11-27.</p></div>"),
        )
        .unwrap();
        let output = run(&[&source]);
        assert_eq!(output[0]["disposition"], "accepted");
        assert_eq!(
            output[0]["page_screening"]["signals"],
            json!(["empty_primary_region", expected_signal])
        );
        let document = &output[1]["document"];
        assert_eq!(document["normalized_text"], "The café reopens 2026-11-27.");
        assert_eq!(
            document["normalizer_version"],
            "semantic-html-marked-region-v1"
        );
        assert_eq!(output[1]["candidate_count"], 1);
        let candidate = &output[2]["candidate"];
        assert_eq!(candidate["value"]["iso_date"], "2026-11-27");
        assert_eq!(candidate["evidence"]["document_version_id"], document["id"]);
        assert_eq!(candidate["evidence"]["quote"], "2026-11-27");
    }
}

#[test]
fn uses_legible_when_no_semantic_landmark_and_extracts_json_ld_and_argument_spans() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("article.html");
    fs::write(
        &source,
        r#"<!doctype html>
<html>
<head>
  <title>Ignored HTML Title</title>
  <meta property="og:title" content="Gallery Expansion">
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
</html>"#,
    )
    .unwrap();

    let output = run(&[&source]);
    assert_eq!(output[0]["disposition"], "accepted");
    assert_eq!(output[0]["metadata"]["title"], "Gallery Expansion");
    assert_eq!(output[0]["metadata"]["title_origin"], "json_ld");
    assert_eq!(output[0]["metadata"]["published_at"], "2026-11-27");
    assert_eq!(output[0]["metadata"]["authors"], json!(["Ada Lovelace"]));
    assert_eq!(
        output[0]["metadata"]["disagreements"][0]["field"],
        "published_at"
    );

    let document = &output[1]["document"];
    assert_eq!(document["normalizer_version"], "legible-html-v1");
    let text = document["normalized_text"].as_str().unwrap();
    assert!(!text.contains("2018-01-01"));

    let relationships: Vec<_> = output[2..]
        .iter()
        .map(|row| &row["candidate"])
        .filter(|candidate| candidate["value"]["kind"] == "relationship")
        .collect();
    assert!(!relationships.is_empty());
    for rel in relationships {
        let args = &rel["argument_evidence"];
        for key in ["subject", "predicate", "object"] {
            let span = &args[key];
            let start = span["start_byte"].as_u64().unwrap() as usize;
            let end = span["end_byte"].as_u64().unwrap() as usize;
            assert_eq!(span["document_version_id"], document["id"]);
            assert_eq!(&text[start..end], span["quote"].as_str().unwrap());
        }
    }
}
