CREATE TABLE acquisition_attempts (
    id                  TEXT PRIMARY KEY,
    source_id           TEXT NOT NULL,
    format              TEXT NOT NULL,
    disposition         TEXT NOT NULL,
    reason              TEXT,
    document_version_id TEXT,
    page_screening_json TEXT,
    metadata_json       TEXT,
    metadata_only       INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE document_versions (
    id                     TEXT PRIMARY KEY,
    source_id              TEXT NOT NULL,
    normalizer_version     TEXT NOT NULL,
    payload_sha256         TEXT NOT NULL,
    normalized_text_sha256 TEXT NOT NULL,
    normalized_text        TEXT NOT NULL,
    created_at             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE candidates (
    id                     TEXT PRIMARY KEY,
    document_version_id    TEXT NOT NULL REFERENCES document_versions(id) ON DELETE CASCADE,
    extractor_version      TEXT NOT NULL,
    rule_id                TEXT NOT NULL,
    kind                   TEXT NOT NULL,
    value_json             TEXT NOT NULL,
    start_byte             INTEGER NOT NULL,
    end_byte               INTEGER NOT NULL,
    quote                  TEXT NOT NULL,
    argument_evidence_json TEXT
);
