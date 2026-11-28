CREATE TABLE reviews (
    id              TEXT PRIMARY KEY,
    question        TEXT NOT NULL,
    total_verified  INTEGER NOT NULL,
    tallies_json    TEXT NOT NULL,
    verified_json   TEXT NOT NULL,
    unresolved_json TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX idx_candidates_doc_version
    ON candidates(document_version_id, start_byte, end_byte);

CREATE INDEX idx_attempts_created
    ON acquisition_attempts(created_at DESC, id ASC);

CREATE INDEX idx_attempts_doc_version
    ON acquisition_attempts(document_version_id);

CREATE INDEX idx_doc_versions_text_hash
    ON document_versions(normalized_text_sha256);
