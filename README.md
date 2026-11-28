## Lux

Lux contains a Rust server and a React frontend. The server loads its
configuration and opens the database; it has no HTTP endpoints yet.

### Prerequisites

- Rust, installed with [rustup](https://rustup.rs). `rust-toolchain.toml` pins the
  toolchain and rustup picks it up automatically.
- Node 22, for the frontend.
- [podman](https://podman.io), to build the container image (`make podman-build`).
- [cargo-deny](https://crates.io/crates/cargo-deny), for `make verify` (the
  dependency audit that CI also runs): `cargo install cargo-deny --locked`.
- [sqlx-cli](https://crates.io/crates/sqlx-cli), optional: only for
  `make migrate-up` (see below).

### Backend

```bash
make run
```

Run it from the repository root. It loads the configuration, opens the SQLite
database, applies the migrations, prints a line and exits. Nothing listens on a
port yet, so the frontend's `/api` proxy has no server behind it.

Settings come from `server/config/config_<env>.yaml`, where `<env>` is the value
of `APP_ENVIRONMENT` (`dev` by default). The database location can be overridden
with the `SQLITE_DSN` environment variable (`sqlite.dsn` in the YAML); `make run`
defaults it to `lux.db`. `config_prd.yaml` leaves it empty on purpose, so set it
when running with `APP_ENVIRONMENT=prd`.

The SQL migrations in `server/migrations` are embedded in the server and applied
at startup, so a fresh checkout needs no extra step. To apply the same files to
the database in `SQLITE_DSN` by hand, install sqlx-cli and run:

```bash
make migrate-up
```

### Frontend

Install dependencies and start Vite:

```bash
make web-install
make web-dev
```

The frontend listens on `http://localhost:5173` and proxies `/api/*` to the Rust
server. The API client can be pointed at another server with
`web/.env.local`:

```bash
VITE_API_BASE_URL=/api
```

Frontend checks:

```bash
make web-lint
make web-typecheck
make web-build
```

### Extraction CLI

Run from the repository root with saved UTF-8 HTML and text files:

```bash
cargo run --locked -p extraction --bin paradis-extract -- saved-page.html notes.txt > results.jsonl
```

Each input emits an `attempt` row with its disposition. Accepted inputs also
emit a `document` row with normalized text and hashes, followed by `candidate`
rows in text order. A candidate includes its rule version and an evidence
quote anchored by half-open UTF-8 byte offsets in the document version.
Skipped and uncertain files emit an attempt with a reason, and the batch continues.

Saved HTML attempts include `page_screening` with the rule version and signals.
Screening checks for empty pages, recognizable soft 404s, consent-only dialogs,
domain parking, and likely JavaScript shells. Shells receive an `uncertain`
disposition. If the selected main region renders empty, the CLI tries a
region marked `itemprop="articleBody"` or `role="main"` before the pruned
body. The attempt and normalizer version record which fallback supplied text.
`--html-body` is the unscreened, unpruned comparison for local snapshots.

The baseline recognizes full ISO and English month-name dates (including
ordinal days), numbers with
supported units, and a small set of explicit relationship phrases such as
`contains` and `is located in`. HTML processing selects semantic content
and drops common boilerplate. It parses saved files without executing scripts
or making network requests. Inputs are capped at 2 MiB per file. The document
row flags truncation if it exceeds 2,000 candidates.
The extractor expands explicit decimal scales such as `$1.2 million` and skips
fraction fragments and ambiguous coordinated clauses.

The saved-file output includes normalized text and the source file path.
Keep the original snapshot available if you need to replay normalization.
Within a batch, an exact normalized-text copy points to its first occurrence
through `exact_text_duplicate_of`. When the same source is observed with a
different document version, `previous_observed_version` identifies the
preceding observation. Each document retains its own source and evidence.

Use a snapshot manifest when archived files represent the same logical source
or come from known reporting lineages:

```bash
target/debug/paradis-extract --snapshot-manifest \
  crates/extraction/benchmarks/provenance/manifest.json > provenance.jsonl
jq 'select(.record_type == "evidence_review") | .current_values' provenance.jsonl
```

The manifest uses `schema_version: 1` and lists snapshots in observation
order. Each has an `id`, a `path` relative to the manifest or absolute, and a
stable `source_id`. Optional `captured_at` records the archive capture time.
`supersedes` points to an earlier snapshot of the same source.
`copied_from` points to a version from a different source. Both inherit a
declared `attestation_id` when one is available. Supply one attestation ID
per known origin of the reporting. An exact-text match is recorded separately.

A manifest can request `metadata_only` retention or set a `retain_until`
deadline in RFC 3339 format. These restrictions withhold the file contents,
normalized documents, and evidence from new output when they apply. Previously
exported JSON Lines files are managed by the caller.

An optional `review` provides a question and candidate selections by
snapshot ID and exact evidence quote. Include `start_byte` when a quote
appears more than once. The report keeps historical evidence, tallies exact
current candidate values once per declared attestation, and flags conflicting
values within a single attestation. Sources with unknown lineage remain
inspectable outside the tally. The supplied selections and lineage determine
the scope of the review.

### RSS and Atom feeds

Extract from saved UTF-8 RSS 2.0 and Atom files:

```bash
target/debug/paradis-extract --feed crates/extraction/benchmarks/feeds/rss.xml \
  crates/extraction/benchmarks/feeds/atom.xml > feed-results.jsonl
```

The CLI emits a feed attempt and item records. It extracts title, summary,
and inline text or HTML content as separate documents. Each document
cites its entry position, field, and publisher item ID when available.
Publication and modification times remain distinct. The raw feed digest
and normalized text digest accompany each version.

Feed links are discovery records for later policy checks. The command uses
`xml:base` and an optional `--base-url URL` when processing a single saved
file. It removes fragments and withholds links that carry credentials or
queries. A pagination record can report `link_query_withheld` without
exporting its cursor. Saved-feed parsing makes zero network requests.

The feed reader accepts up to 2 MiB per file, 64 levels of XML nesting,
and 100,000 XML events. It rejects DTDs and undeclared entities. Output
is limited to 100 entries and 20 links per entry.

### Hacker News live channels

To compare the approved Hacker News RSS feed with the official API:

```bash
target/debug/paradis-extract --hacker-news-rss 15 > hn-rss.jsonl
target/debug/paradis-extract --hacker-news 15 > hn-api.jsonl
python3 scripts/compare-hacker-news.py hn-api.jsonl hn-rss.jsonl --route rss
```

The RSS route applies the source's robots policy before requesting the
feed. Titles and numeric story IDs carry their field locators. Raw
responses stay in memory. Feed links supply identity metadata only for
this route.

To extract from the official Hacker News API:

```bash
cargo run --locked -p extraction --bin paradis-extract -- --hacker-news 10 > hn-results.jsonl
```

The limit applies to unique top item IDs examined, with a maximum of 25.
The output records the index and item requests, accepted titles, and
candidates linked to the `/title` field. The API can include jobs or unavailable
items in its top list, which are recorded as skipped. The client requests only
the API host over HTTPS and stops the batch on access restrictions or rate
limits. Raw responses stay in memory. The output contains normalized titles,
evidence, and provenance metadata. Linked articles and comments are outside
this run.
An index request failure emits a JSON Lines attempt record and exits nonzero.

To compare with the approved Hacker News front page, capture the same limit
through both routes and score the saved title records:

```bash
cargo build --locked -p extraction --bin paradis-extract
target/debug/paradis-extract --hacker-news 15 > hn-api.jsonl
target/debug/paradis-extract --hacker-news-page 15 > hn-page.jsonl
python3 scripts/compare-hacker-news.py hn-api.jsonl hn-page.jsonl
```

The front-page route requests `robots.txt` first. It checks the configured
HTTPS origin and paths, honors the applicable crawl delay, and records the
robots decision and its expiry. A denied or deferred attempt ends the run.
The accepted HTML request emits up to the requested limit of normalized
scored story titles. Each title has a story ID, a CSS field locator, and
versioned evidence. The response bytes stay in memory.

### Extraction benchmark

```bash
cargo build --locked -p extraction --bin paradis-extract
python3 scripts/benchmark-extraction.py crates/extraction/benchmarks/synthetic/corpus.json
python3 scripts/benchmark-extraction.py crates/extraction/benchmarks/page-screening.json
```

The scorer runs the same CLI on each saved input and its annotated reference
text. It reports content token overlap, dispositions, evidence validity, and
exact candidate precision and recall by field and source type. Empty metric
denominators produce `null`. The synthetic corpus exercises the scoring path
and includes known omissions. Use a separate representative labeled corpus to
estimate extraction accuracy.

For saved HTML, the report compares the default cleaner with a control that
uses the same renderer without main-content pruning. The recorded durations
include CLI startup, normalization, candidate extraction, and serialization.

Mozilla Readability can be added as an offline comparator. For this optional
run, use Node 22.13 or newer in the 22.x series, or Node 24 or newer:

```bash
benchmark_deps=$(mktemp -d)
npm install --prefix "$benchmark_deps" --ignore-scripts @mozilla/readability@0.6.0 jsdom@29.1.1
python3 scripts/benchmark-extraction.py crates/extraction/benchmarks/synthetic/corpus.json \
  --readability-node-modules "$benchmark_deps/node_modules"
```

Readability selects content from saved HTML with scripts and network loading
disabled. The same Rust renderer and candidate extractor process its output.
The report includes the installed comparator versions and its elapsed time
includes the Node process.

The Rust Readability comparison tool has its own manifest and lockfile.
Build it to compare both Rust ports and, on Linux with GNU time, measure the
maximum peak RSS of the processes used by each method:

```bash
cargo build --locked --release -p extraction --bin paradis-extract
cargo build --locked --release --manifest-path scripts/rust-readability-baseline/Cargo.toml \
  --target-dir target/readability-comparator
python3 scripts/benchmark-extraction.py /path/to/prepared/articles/corpus.json \
  --extractor target/release/paradis-extract \
  --readability-node-modules "$benchmark_deps/node_modules" \
  --rust-readability-baseline target/readability-comparator/release/rust-readability-baseline \
  --measure-peak-rss
```

The [first benchmark report](docs/extraction-benchmark-results.md) records
published corpus results and the separately scored Hacker News title sample.

### Offline relationship comparison

Use a Python environment with spaCy 3.8 and `en_core_web_sm` 3.8.0 for
the optional syntax comparison. The benchmark report has the install
commands and corpus versions.

```bash
target/debug/paradis-extract -- saved-page.html |
  jq -j 'select(.record_type == "document") | .document.normalized_text' > normalized.txt
python scripts/parse-dependencies.py normalized.txt > dependencies.json
target/debug/paradis-extract --dependencies dependencies.json -- saved-page.html
```

The Rust CLI checks the normalized text hash, token byte ranges, and dependency
trees before emitting candidates. Relationship candidates record the parser and
model alongside the Rust rule version and evidence quote. Dates and quantities
use the same surface rules in both modes. Keep `--html-body` consistent
across both CLI calls when comparing unpruned HTML.

For an offline Stanford CoreNLP 4.6.0 OpenIE comparison (requiring Java 11+ with
`java.desktop` and a local CoreNLP 4.6.0 distribution):

```bash
python3 scripts/carb-predictions.py /path/to/sentences.txt \
  --corenlp-dir /path/to/stanford-corenlp-4.6.0 \
  --corenlp-view anchored \
  --write-openie-evidence openie-evidence.jsonl
```

### Layout

```
.
├── server/        Rust API server (crate lux-server)
│   ├── config/        config_dev.yaml, config_prd.yaml
│   ├── migrations/    SQL migrations
│   └── src/           main.rs, config.rs, db.rs
├── crates/
│   ├── example/   example feature as its own crate (type, rules, SQL); not used by the server yet
│   └── extraction/ saved-document extraction and its CLI
├── web/           React frontend (Vite)
├── docs/
│   └── style.md   Rust style guide
├── scripts/
│   └── rename-project.sh   renames the project (see "Using this template")
├── Cargo.toml     workspace root: shared dependencies and lints
└── Makefile
```

### Code style

Rust code follows [docs/style.md](docs/style.md), which also maps out where things
live in `server/src`.

### Using this template

To rename the project from Lux to your own name, run this from the repository root:

```bash
scripts/rename-project.sh acme
```

The name is lowercase letters, digits, `-` and `_`. The script renames the crate and
binary (`acme-server`), the library (`acme_server`), the web package, the default
`acme.db` file name, and the visible "Lux" titles. It then refreshes
`Cargo.lock` and runs `cargo fmt`. It needs GNU sed (on macOS, install `gnu-sed`) and
is safe to run twice. Review `git diff` afterwards and run `make test`.

This template does not include a `LICENSE`; add the one you want.
