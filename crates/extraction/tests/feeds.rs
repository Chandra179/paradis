//! End-to-end feed parsing, provenance, and resource-limit checks.

#![allow(clippy::unwrap_used)]

use std::{collections::HashMap, fs, path::Path, process::Command};

use extraction::sha256_hex;
use serde_json::Value;

fn run(options: &[&str], files: &[&Path]) -> Vec<Value> {
    let result = Command::new(env!("CARGO_BIN_EXE_paradis-extract"))
        .arg("--feed")
        .args(options)
        .args(files)
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
fn anchors_rss_and_atom_fields_to_their_own_documents() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks/feeds");
    let rss = folder.join("rss.xml");
    let atom = folder.join("atom.xml");
    let output = run(&[], &[&rss, &atom]);
    let replay = run(&[], &[&rss, &atom]);
    let stable = |rows: Vec<Value>| {
        rows.into_iter()
            .filter(|r| r["record_type"] != "feed_attempt")
            .collect::<Vec<_>>()
    };
    assert_eq!(stable(output.clone()), stable(replay));
    let attempts: Vec<_> = output
        .iter()
        .filter(|r| r["record_type"] == "feed_attempt")
        .collect();
    assert_eq!(attempts[0]["format"], "rss2");
    assert_eq!(attempts[0]["entries_parsed"], 2);
    assert!(attempts[0]["feed_id"].is_null());
    assert_eq!(attempts[1]["format"], "atom");
    assert_eq!(attempts[1]["feed_id"], "urn:gallery:updates");

    let documents: Vec<_> = output
        .iter()
        .filter(|r| r["record_type"] == "document")
        .collect();
    assert_eq!(documents.len(), 7);
    assert_eq!(documents[0]["source_record"]["item_id"], "workshop-17");
    assert_eq!(documents[0]["source_record"]["locator"], "title");
    assert_eq!(
        documents[0]["source_record"]["published_at"],
        "2026-11-27T08:00:00Z"
    );
    assert!(documents[0]["source_record"].get("updated_at").is_none());
    assert_eq!(documents[1]["source_record"]["locator"], "summary");
    assert_eq!(documents[2]["source_record"]["locator"], "content");
    assert!(documents[3]["source_record"]["item_id"].is_null());
    assert_eq!(
        documents[4]["document"]["normalized_text"],
        "Gallery <West> opens on 2026-11-27."
    );
    assert_eq!(
        documents[4]["source_record"]["published_at"],
        "2026-11-24T09:00:00Z"
    );
    assert_eq!(
        documents[4]["source_record"]["updated_at"],
        "2026-11-26T12:00:00Z"
    );
    let digests = [
        sha256_hex(&fs::read(rss).unwrap()),
        sha256_hex(&fs::read(atom).unwrap()),
    ];
    let versions: HashMap<_, _> = documents
        .iter()
        .map(|row| {
            let index = usize::try_from(row["attempt_index"].as_u64().unwrap()).unwrap() - 1;
            let doc = &row["document"];
            assert_eq!(doc["payload_sha256"], digests[index]);
            (
                doc["id"].as_str().unwrap(),
                doc["normalized_text"].as_str().unwrap(),
            )
        })
        .collect();
    let candidates: Vec<_> = output
        .iter()
        .filter(|r| r["record_type"] == "candidate")
        .collect();
    assert_eq!(candidates.len(), 8);
    for row in candidates {
        let evidence = &row["candidate"]["evidence"];
        let version = evidence["document_version_id"].as_str().unwrap();
        let start = usize::try_from(evidence["start_byte"].as_u64().unwrap()).unwrap();
        let end = usize::try_from(evidence["end_byte"].as_u64().unwrap()).unwrap();
        assert_eq!(
            versions[version].get(start..end),
            evidence["quote"].as_str()
        );
    }

    let links: Vec<_> = output
        .iter()
        .filter(|r| r["record_type"] == "feed_link")
        .collect();
    assert!(
        links
            .iter()
            .any(|r| r["url"] == "https://gallery.example/notices/west-wing")
    );
    assert!(links.iter().any(|r| r["relation"] == "next"
        && r["reason"] == "link_query_withheld"
        && r["url"].is_null()));
    assert!(
        links
            .iter()
            .any(|r| r["reason"] == "unsupported_link_scheme")
    );
    let exported = serde_json::to_string(&output).unwrap();
    for withheld in [
        "not-exported",
        "hidden script contents",
        "contact@gallery.example",
        "#hours",
    ] {
        assert!(!exported.contains(withheld), "{withheld}");
    }
}

#[test]
fn rejects_dtds_malformed_feeds_and_xml_over_budget() {
    let folder = tempfile::tempdir().unwrap();
    let cases = [
        (
            "<!DOCTYPE rss><rss version=\"2.0\"><channel/></rss>".to_owned(),
            "xml_doctype_disallowed",
        ),
        (
            "<rss version=\"2.0\"><channel></rss>".to_owned(),
            "invalid_feed_xml",
        ),
        (
            format!("<rss version=\"2.0\">{}", "<section>".repeat(65)),
            "xml_depth_limit",
        ),
        (
            format!(
                "<rss version=\"2.0\">{} </rss>",
                "<section/>".repeat(100_001)
            ),
            "xml_event_limit",
        ),
        (
            "<rss version=\"2.0\"><channel><title>&custom;</title></channel></rss>".to_owned(),
            "unsupported_xml_entity",
        ),
        (
            "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><rss version=\"2.0\"/>".to_owned(),
            "unsupported_xml_encoding",
        ),
    ];
    let files: Vec<_> = cases
        .iter()
        .enumerate()
        .map(|(index, (xml, _))| {
            let file = folder.path().join(format!("{index}.xml"));
            fs::write(&file, xml).unwrap();
            file
        })
        .collect();
    let output = run(&[], &files.iter().map(|f| f.as_path()).collect::<Vec<_>>());
    assert_eq!(output.len(), cases.len());
    for (row, (_, reason)) in output.iter().zip(cases) {
        assert_eq!(row["record_type"], "feed_attempt");
        assert_eq!(row["disposition"], "skipped");
        assert_eq!(row["reason"], reason);
    }
}

#[test]
fn resolves_relative_feed_links_from_an_explicit_base() {
    let folder = tempfile::tempdir().unwrap();
    let input = folder.path().join("feed.xml");
    fs::write(
        &input,
        r#"<rss version="2.0"><channel><title>Updates</title>
      <item><title>Open for 4 hours</title><link>events/today#hours</link></item>
      </channel></rss>"#,
    )
    .unwrap();
    let output = run(
        &["--base-url", "https://gallery.example/news/feed.xml"],
        &[&input],
    );
    assert_eq!(
        output[0]["base_url"],
        "https://gallery.example/news/feed.xml"
    );
    assert!(
        output
            .iter()
            .any(|r| r["url"] == "https://gallery.example/news/events/today")
    );
}
