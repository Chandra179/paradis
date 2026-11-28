//! End-to-end HTTP and SQLite integration tests for `paradis-server`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use paradis_server::{app, db};
use serde_json::{Value, json};
use tokio::net::TcpListener;

async fn spawn_test_app() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();
    db::migrate(&pool).await.unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app(pool)).await.unwrap();
    });
    (format!("http://{addr}"), dir)
}

#[tokio::test]
async fn health_extract_detail_and_review_round_trip() {
    let (base, _dir) = spawn_test_app().await;
    let client = reqwest::Client::new();

    let health: Value = client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");

    let ready: Value = client
        .get(format!("{base}/ready"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ready["status"], "ready");

    let extracted: Value = client
        .post(format!("{base}/documents/extract"))
        .json(&json!({
            "source_id": "gallery-note",
            "format": "html",
            "content": "<main><p>The gallery is located in the west wing. Inspection on 2026-11-27.</p></main>"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(extracted["attempt"]["disposition"], "accepted");
    let doc_id = extracted["document"]["id"].as_str().unwrap().to_owned();
    assert!(
        !extracted["document"]["candidates"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let list: Value = client
        .get(format!("{base}/documents"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], doc_id);

    let detail: Value = client
        .get(format!("{base}/documents/{doc_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["source_id"], "gallery-note");

    let review: Value = client
        .post(format!("{base}/reviews"))
        .json(&json!({
            "question": "Where is the gallery?",
            "selections": [
                {
                    "document_version_id": doc_id,
                    "quote": "The gallery is located in the west wing"
                }
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(review["total_verified"], 1);
    assert_eq!(
        review["tallies"]["The gallery is located in the west wing"],
        1
    );

    let restricted: Value = client
        .post(format!("{base}/documents/extract"))
        .json(&json!({
            "source_id": "restricted-note",
            "format": "text",
            "content": "Private report 2026-11-27.",
            "metadata_only": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restricted["attempt"]["disposition"], "skipped");
    assert_eq!(restricted["attempt"]["reason"], "metadata_only");
    assert!(restricted["document"].is_null());

    let attempts: Value = client
        .get(format!("{base}/attempts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(attempts.as_array().unwrap().len(), 2);

    let reviews: Value = client
        .get(format!("{base}/reviews"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reviews.as_array().unwrap().len(), 1);
    assert_eq!(reviews[0]["question"], "Where is the gallery?");
    assert_eq!(reviews[0]["total_verified"], 1);
}
