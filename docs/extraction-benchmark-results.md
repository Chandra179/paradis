# First extraction benchmark

The 27 November 2026 run evaluates the Rust `surface-v1` extractor. The article, CaRB, and Hacker News results measure different outcomes and use separate denominators.

## Article body extraction

The comparison ran on all 181 saved pages in the pinned Zyte article benchmark. Scores use multiset token overlap against the published reference bodies. Durations are sums of whole-process elapsed time across the corpus, including candidate emission and separate Node startup for Readability. Peak memory was not measured. [Zyte article benchmark](https://github.com/scrapinghub/article-extraction-benchmark/tree/4a3bc979f76c0df73cb95fe272e2fc1b96f9f010)

| Method | Token precision | Token recall | Token F1 | Total elapsed |
| --- | ---: | ---: | ---: | ---: |
| Full body | 50.77% | 100.00% | 67.35% | 34.51 s |
| Rust semantic pruning | 70.78% | 94.73% | 81.02% | 32.94 s |
| Mozilla Readability 0.6.0 and jsdom 29.1.1 | 92.36% | 98.17% | 95.18% | 136.99 s |

The Rust cleaner accepted 178 useful inputs. The corpus contains 181 useful inputs and zero unusable inputs, so it can estimate useful-document retention for these snapshots. Readability's DOM parser failed on one snapshot with malformed CSS. That page remains in the denominator with empty comparator output. All emitted spans from the three methods passed mechanical evidence checks.

### Rust Readability and marked-region comparison

The 28 November replay uses the same pinned 181 snapshots and article-body
labels. The production cleaner now tries a region marked
`itemprop="articleBody"` or `role="main"` when its first selection renders
empty. The marked-region fallback supplied text on the three pages that
previously relied on the pruned body. Their token precision moved from
78.52% to 78.84%, from 9.73% to 100%, and from 61.76% to 62.00%,
respectively. Recall was 100% in all three pairs. The other 178 accepted
document versions reproduced their previous IDs. [Pinned Zyte article benchmark](https://github.com/scrapinghub/article-extraction-benchmark/tree/4a3bc979f76c0df73cb95fe272e2fc1b96f9f010)

The same Rust plain-text renderer processed the full body and the selected
HTML from Mozilla Readability, `dom_smoothie`, and Legible. The main CLI
also performed its usual snapshot screening for the production cleaner row.
The Rust comparison executables were release builds and Mozilla Readability
ran with Node 25.6.1. Wall times sum per-snapshot execution, including
startup and the rendering pass. Peak RSS is the largest single-process
reading for the method. The Rust ports run in an independent benchmark
workspace. [dom_smoothie 0.20.0](https://docs.rs/dom_smoothie/0.20.0/dom_smoothie/) · [Legible 0.5.1](https://docs.rs/crate/legible/0.5.1)

| Method | Accepted | Token precision | Token recall | Token F1 | Total wall time | Peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Full body | 181 | 50.77% | 100.00% | 67.35% | 8.16 s | 21,908 KiB |
| Rust semantic cleaner with marked regions | 181 | 70.78% | 95.83% | 81.42% | 7.93 s | 21,948 KiB |
| Mozilla Readability 0.6.0 and jsdom 29.1.1 | 180 | 92.36% | 98.17% | 95.18% | 119.14 s | 311,728 KiB |
| `dom_smoothie` 0.20.0 | 181 | 92.09% | 99.54% | 95.67% | 9.04 s | 21,992 KiB |
| Legible 0.5.1 | 172 | 92.93% | 95.89% | 94.39% | 10.17 s | 21,884 KiB |

Mozilla recorded one parser failure. Legible returned no main content for
nine labeled articles. All these pages remained in the denominator. All
emitted evidence spans passed the offset checks. In the small synthetic
sample, each Readability method retained the three labeled article quotes
and also returned navigation text from the script-only shell fixture. The
normal pipeline's screening withheld that shell.

`dom_smoothie` led in F1 on this corpus. Its dependency graph has four
MPL-2.0 crates outside the extraction workspace's allowlist. Legible passed
the policy check on its own dependency graph. Both ports are scoped to the
offline comparison. [Reproduction commands](../README.md#extraction-benchmark)

## Saved HTML page screening

The 28 November 2026 replay compared the previous CLI with the default Rust pipeline using `html-screen-v1`. The saved fixtures label useful content, recognizable soft 404s, consent-only dialogs, parked domains, missing bodies, empty and blank input, and JavaScript shells. They pair some unusable pages with useful notices and documentation that contain similar wording. [Frozen screening fixtures](../crates/extraction/benchmarks/page-screening.json)

| 18 saved fixtures | Previous CLI | Screened CLI |
| --- | ---: | ---: |
| Useful pages accepted | 8 of 8 | 8 of 8 |
| Unusable pages withheld | 5 of 10 | 9 of 10 |
| Exact disposition matches | 11 of 18 | 17 of 18 |
| Exact disposition and reason matches | 8 of 18 | 17 of 18 |

The screened CLI skipped six pages and marked three suspected JavaScript shells uncertain. All nine withheld pages were labeled unusable. All eight useful pages retained their labeled text. The missed fixture was an expired page whose heading and message differed from the recognized soft-error patterns. The corpus is synthetic and small, so these counts describe the fixtures.

The same pinned Zyte corpus improved from 178 accepted articles out of 181 to 181 out of 181. Screening alone preserved the original 178 documents byte for byte. The other three previously returned empty after semantic selection. The cleaner recovered all labeled body tokens on those pages by falling back to the pruned body. Precision on the recovered pages was 78.52%, 9.73%, and 61.76%, which reflects extra text retained by the fallback. A single batch of the debug builds took 6.10 seconds and 30,692 KiB peak RSS before the change, and 6.59 seconds and 30,032 KiB after it. [Zyte article benchmark](https://github.com/scrapinghub/article-extraction-benchmark/tree/4a3bc979f76c0df73cb95fe272e2fc1b96f9f010)

The screening result describes the contents of a saved snapshot. Transport status, robots decisions, and source permissions are acquisition checks. Repeat the screening run with `python3 scripts/benchmark-extraction.py crates/extraction/benchmarks/page-screening.json`. The report includes the expected and observed disposition, reason, and screening signals for each file.

## Open relationships

CaRB's full test input contained 641 sentences and 2,715 annotated tuples. The extractor proposed three relationships and matched zero gold tuples under strict equality of sentence, predicate, and all arguments. This strict count measures exact compatibility with the published labels. The dataset files were verified against the blobs in the pinned checkout. The initial run reported only exact counts. [CaRB benchmark and scorer](https://github.com/dair-iitd/CaRB/tree/024e0e91951eea1e6d7825a13ec303cae909e67b)

The published Stanford OpenIE prediction file provides a reference for candidate coverage. Using CaRB's sentence-key normalization and exact equality on predicate and arguments, Stanford produced 3,676 aligned tuples across 531 test sentences and matched 37 gold tuples. A further 16,851 prediction rows did not map to a test sentence key and were excluded. The Rust extractor produced a prediction on three sentences. The Stanford numbers describe the published outputs. [Stanford prediction file](https://github.com/dair-iitd/CaRB/blob/024e0e91951eea1e6d7825a13ec303cae909e67b/system_outputs/test/stanford_output.txt)

## Hacker News titles

The official top-stories API returned 20 titles at 21:17 Jakarta time. The [frozen title sample](../crates/extraction/benchmarks/hacker-news-2026-11-27.json) records item IDs, document version IDs, normalized titles, and manual labels. All 20 item requests and the index request returned HTTP 200. Extraction on the frozen titles reproduced the live candidate values and evidence quotes. [Hacker News API](https://github.com/HackerNews/API)

| Field | Matches | False positives | Misses |
| --- | ---: | ---: | ---: |
| Full dates | 0 | 0 | 1 |
| Quantities | 1 | 0 | 1 |
| Relationships | 0 | 0 | 3 |

Labels cover explicit full calendar dates, literal number and unit mentions, and direct asserted clauses with a subject, predicate, and object or noun complement. The two endpoints of the range were labeled as mentions. The current output schema has no field for the relationship between endpoints. The date miss contained an ordinal day. All six annotated quotes were available to the extractor, and the one emitted evidence span passed version and byte-offset checks. A single top-stories capture and single annotator limit generalization.

The subsequent `surface-v2` replay detected the ordinal date and retained the quantity match. It produced two matches out of six labels, zero false positives, and zero invalid evidence spans. This replay uses the sample that exposed the miss. Separate regression fixtures verify both day orders and reject malformed ordinal suffixes.

### Live API and direct HTTP comparison

On 28 November 2026 at 10:11–10:12 Jakarta time the CLI captured the top 15
items from the official API and up to 15 scored story titles from the approved
Hacker News front page. The API returned HTTP 200 for its index and all 15
items. The page route returned HTTP 200 for both robots and the page, and
spaced the requests 30 seconds apart according to the crawl delay.

| Comparison | Count |
| --- | ---: |
| Titles from each route | 15 |
| Shared story IDs | 15 |
| Exact normalized title matches | 15 |
| Matching candidate sets on equal titles | 15 |
| Emitted candidates from each route | 1 |

The comparison verified document text digests and each candidate's quote and
byte range. Matching candidate sets include titles where neither route
emitted a candidate. The measured agreement applies to titles from this
single observation.
[Hacker News API](https://github.com/HackerNews/API)

The page route uses `robotxt` 0.7.0 for rule matching. Controlled fixtures
cover group matching and precedence, unavailable policies, restricted and
throttled responses, cache expiry, redirects outside the approved scope,
per-destination robots decisions, and body bounds. The tests observe whether
the transport is called for each destination. A policy limit or denied route
produces an attempt record and stops retrieval. [RFC 9309](https://www.rfc-editor.org/rfc/rfc9309.html) · [robotxt 0.7.0](https://docs.rs/robotxt/0.7.0/robotxt/)

## Saved feeds and Hacker News RSS

The saved RSS 2.0 and Atom fixtures produced seven field documents from
three entries and eight evidence-backed candidates. All eight quotes matched
the byte slices of their cited document versions. Replay reproduced the
ordered field, candidate, and discovery records. The fixture retained
distinct Atom publication and update times, resolved a nested `xml:base`,
and recorded a missing RSS GUID as unknown. [RSS fixture](../crates/extraction/benchmarks/feeds/rss.xml) · [Atom fixture](../crates/extraction/benchmarks/feeds/atom.xml)

Five discovery records included two HTTP(S) targets, two withheld queries
and one unsupported scheme. The Atom pagination link is one of the
withheld queries. The offline route makes zero requests. Controlled
fixtures rejected DTDs, malformed XML, unsupported encodings and entities,
and XML over the event or nesting budgets. The extraction dependency audit
passed with `feed-rs` 3.1.0 and `quick-xml` 0.42.0. [RSS 2.0 specification](https://www.rssboard.org/rss-specification) · [Atom RFC 4287](https://www.rfc-editor.org/rfc/rfc4287.html) · [feed-rs 3.1.0](https://docs.rs/feed-rs/3.1.0/feed_rs/)

On 28 November 2026 at 10:53–10:54 Jakarta time, the approved Hacker News
RSS route returned 15 titles and the API returned 15. All 15 story IDs,
normalized titles and candidate sets matched. Each route emitted one
candidate with a verified evidence span. The RSS route used two HTTP
requests, including robots, and honored the site's 30-second crawl delay.
The API used 16 requests, including its index. All requests returned HTTP
200. The result measures title agreement for this single observation.
[Hacker News RSS](https://news.ycombinator.com/rss) · [Hacker News API](https://github.com/HackerNews/API)

The first live RSS attempt exposed a mismatch in the field used for
identity. The adapter now reads the declared RSS `comments` element
for the numeric story ID and records its locator. A regression fixture
covers a self-post with identical item and comments links, an item
linking to a different HN story, and conflicting GUID and comments IDs.
The live capture exported normalized titles, story IDs, evidence and
request metadata. [Feed commands](../README.md#hacker-news-live-channels)

## Provenance, corrections, and retention

The offline snapshot manifest exercises one reporting question over ten
observations. The gallery's `3 kg` notice appears as the original, an
identical replay, an exact copy, and an edited reprint. The gallery later
publishes a correction to `5 kg`. Independent records report `5 kg` and
`7 kg`, and an additional copy has unknown lineage. [Frozen snapshot manifest](../crates/extraction/benchmarks/provenance/manifest.json)

| Review outcome | Observed result |
| --- | --- |
| Historical evidence | Four observations of the original `3 kg` report, each with its own citation |
| Current `5 kg` candidate | Two declared reporting lineages |
| Current `7 kg` candidate | One declared reporting lineage and one observation with unknown lineage |
| Retention restrictions | Two attempts with metadata only and zero emitted quotes or documents |

All eight selected spans resolved to UTF-8 byte ranges in their cited
document versions, and an offline replay produced the same review. The
original and correction share a stable source ID and retain distinct
document version IDs. The edited reprint has a declared copy link even
though its text hash differs. A second fixture verifies abstention on an
ambiguous quote and leaves a reporting lineage unresolved when it supplies
two different current values.

The fixture supplies known reporting lineage and the exact evidence to
review. These results measure preservation and counting under those
declarations. [End-to-end provenance checks](../crates/extraction/tests/provenance.rs)

## Dependency relationship comparison

The 28 November 2026 run compared the current Rust surface rules with an optional Rust syntax rule on identical normalized text. A local spaCy 3.8.18 parser with the English CPU model `en_core_web_sm` 3.8.0 supplied sentence boundaries and dependency trees. The rule extracts a root verb with an explicit subject and direct object, or a form of *be* with a predicate complement. It checks for conditions, negation, modality, and other attachments that would need further interpretation. Each emitted relationship records the rule, parser, model, and a quote with checked UTF-8 offsets. [English model release](https://github.com/explosion/spacy-models/releases/tag/en_core_web_sm-3.8.0)

| CaRB test method | Candidates | Exact matches | Partial precision | Partial recall | Partial F1 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust surface rules v3 | 2 | 0 | 15.98% | 0.038% | 0.075% |
| Rust dependency rules v1 | 11 | 5 | 89.88% | 0.597% | 1.185% |

The benchmark scored 641 sentences and 2,715 gold tuples. Both methods used one uncalibrated confidence value. Partial scores use the upstream default matcher at that single threshold. The high partial precision for the syntax method describes 11 proposals, and partial recall stayed below 1%. Published Stanford OpenIE outputs remain a useful coverage reference because they reach 531 of 641 test sentences. [CaRB scorer](https://github.com/dair-iitd/CaRB/blob/024e0e91951eea1e6d7825a13ec303cae909e67b/carb.py)

| Saved corpus | Labels | Surface exact matches | Dependency exact matches |
| --- | ---: | ---: | ---: |
| Synthetic relationships | 4 | 2 | 3 |
| Frozen Hacker News relationships | 3 | 0 | 0 |

The dependency rules also returned the unlabeled synthetic relationship *The new shelves support 30 kg*, which counts as one false positive under strict fixture scoring. They missed *The gallery is located in the west wing*, which the surface rules found. Neither method recovered a labeled Hacker News relationship. Both kept the same date and quantity results, and the saved-corpus checks found zero invalid evidence spans. The current evidence supports keeping syntax as an offline experiment. Wider-coverage Open IE and broader annotated web samples are still needed to evaluate general relationship extraction.

On *The café contains paintings and sculptures*, the parser and rule retained both coordinated nouns as the object. On *The café contains paintings and documents the restoration*, the parser tagged *documents* as a noun and the rule abstained. With a hand-corrected tree marking *documents* as a second verb, the same rule recovered *The café / contains / paintings*. This case distinguishes a parsing error from a limitation of the extraction rule.

### Stanford OpenIE comparison

The 28 November 2026 OpenIE run evaluated Stanford CoreNLP `4.6.0` as an offline comparator over the same Rust-normalized `plain-text-v1` document text (`scripts/corenlp_openie.py` and `scripts/carb-predictions.py`). CoreNLP ran with `-annotators tokenize,ssplit,pos,lemma,depparse,natlog,openie`, `-ssplit.eolonly true`, `-openie.resolve_coref false`, and `-openie.triple.strict true` in 64-line `-filelist` chunks inside an offline Java `11.0.20` container (`--network=none`). The adapter counts all raw model proposals first, then withholds any tuple whose subject, predicate, or object tokens do not match a contiguous, non-overlapping original token span in the source line. [Stanford CoreNLP](https://stanfordnlp.github.io/CoreNLP/)

| CaRB test method | Sentences | Candidates | Exact matches | Partial precision | Partial recall | Partial F1 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust surface rules v3 | 2 | 2 | 0 | 15.98% | 0.038% | 0.075% |
| Rust dependency rules v1 | 11 | 11 | 5 | 89.88% | 0.597% | 1.185% |
| CoreNLP 4.6.0 (raw proposals) | 552 | 5,887 | 33 | 16.71% | 25.18% | 20.09% |
| CoreNLP 4.6.0 (source-anchored) | 478 | 1,724 | 20 | 39.30% | 16.14% | 22.89% |

Across the 641 CaRB test sentences (2,715 gold tuples), source-anchoring withheld 4,163 of 5,887 raw proposals (`70.7%`) for `nonliteral_argument`, with zero failures for `invalid_token_span` or `overlapping_arguments`. Requiring each argument to trace back to exact source bytes more than doubled CaRB partial-match precision (`16.71%` to `39.30%`) and raised partial F1 (`20.09%` to `22.89%`), while retaining coverage across 478 sentences. All 1,724 anchored CaRB candidates (6,896 sentence and argument spans) passed `document_version_id` and UTF-8 byte-slice verification against the normalized document.

On the saved corpora (using non-empty Rust-normalized lines):
- **Synthetic relationships (4 labels across 3 non-empty fixtures):** CoreNLP 4.6.0 emitted 13 raw proposals and all 13 anchored cleanly (`0` withheld), matching 1 exact label (*Portable battery pack / weighs / 12 kg*). On the other three labeled sentences CoreNLP omitted leading determiners (*The*), and on *The gallery may contain a library* it emitted *gallery / may contain / library* despite the modal *may*.
- **Frozen Hacker News titles (3 labels across 20 titles):** CoreNLP 4.6.0 emitted 28 raw proposals across 10 titles, withheld 15 as `nonliteral_argument`, and anchored 13 across 6 titles (`0` exact matches against the manual labels due to possessive tokenization, dropped prepositional tails, or reordered spans).

## Reproduction

Build the CLI and replay the frozen Hacker News titles:

```bash
cargo build --locked -p extraction --bin paradis-extract
python3 scripts/benchmark-extraction.py crates/extraction/benchmarks/hacker-news-2026-11-27.json
```

To repeat the article comparison, use the pinned Zyte checkout and the Readability package versions in the [benchmark instructions](../README.md#extraction-benchmark). Preparation copies saved HTML locally and reads the published article body as reference text. The article corpus supplies body labels, so semantic precision and recall are undefined for those fixtures. [Zyte benchmark checkout](https://github.com/scrapinghub/article-extraction-benchmark/tree/4a3bc979f76c0df73cb95fe272e2fc1b96f9f010)

```bash
benchmark_data=$(mktemp -d)
python3 scripts/prepare-article-benchmark.py /path/to/zyte-checkout "$benchmark_data/articles"
python3 scripts/benchmark-extraction.py "$benchmark_data/articles/corpus.json" \
  --readability-node-modules "$benchmark_deps/node_modules"
```

Export CaRB candidates with the checked-out test sentence file. The TSV follows the upstream tabbed prediction format. Each candidate has confidence `1` because this extractor has no calibrated confidence. [CaRB evaluation instructions](https://github.com/dair-iitd/CaRB/tree/024e0e91951eea1e6d7825a13ec303cae909e67b#evaluating-an-open-ie-extractor)

```bash
python3 scripts/carb-predictions.py /path/to/carb-checkout/data/test.txt > predictions.tsv
python3 scripts/carb-predictions.py /path/to/carb-checkout/data/test.txt \
  --gold /path/to/carb-checkout/data/gold/test.tsv \
  --stanford-output /path/to/carb-checkout/system_outputs/test/stanford_output.txt
```

To rerun the dependency comparison, install the parser for offline use. The run used Python 3.13.5.

```bash
syntax_env=$(mktemp -d)
uv venv --python 3.13 "$syntax_env"
uv pip install --python "$syntax_env/bin/python" spacy==3.8.18 \
  https://github.com/explosion/spacy-models/releases/download/en_core_web_sm-3.8.0/en_core_web_sm-3.8.0-py3-none-any.whl
python3 scripts/benchmark-extraction.py crates/extraction/benchmarks/synthetic/corpus.json \
  --dependency-python "$syntax_env/bin/python"
python3 scripts/benchmark-extraction.py crates/extraction/benchmarks/hacker-news-2026-11-27.json \
  --dependency-python "$syntax_env/bin/python"
carb_checkout=/path/to/carb-checkout
python3 scripts/carb-predictions.py "$carb_checkout/data/test.txt" \
  --dependency-python "$syntax_env/bin/python" \
  --gold "$carb_checkout/data/gold/test.tsv"
```

For the upstream partial-match run, export each local method as TSV by omitting
`--gold`. The modern Python environment needs a compatibility alias for a
legacy scikit-learn import in CaRB. The alias leaves the scoring code intact.

```bash
uv pip install --python "$syntax_env/bin/python" \
  docopt==0.6.2 ipdb==0.13.13 nltk==3.10.4 scikit-learn==1.9.1
export NLTK_DATA="$syntax_env/nltk-data"
"$syntax_env/bin/python" -m nltk.downloader -q -d "$NLTK_DATA" stopwords
for method in surface dependency; do
  options=()
  if [ "$method" = dependency ]; then
    options=(--dependency-python "$syntax_env/bin/python")
  fi
  python3 scripts/carb-predictions.py "$carb_checkout/data/test.txt" \
    "${options[@]}" > "$syntax_env/$method.tsv"
  "$syntax_env/bin/python" -c '
import pathlib, runpy, sys
import sklearn.preprocessing as preprocessing
sys.modules["sklearn.preprocessing.data"] = preprocessing
checkout = pathlib.Path(sys.argv.pop(1)).resolve()
sys.path.insert(0, str(checkout))
runpy.run_path(str(checkout / "carb.py"), run_name="__main__")
' "$carb_checkout" --gold="$carb_checkout/data/gold/test.tsv" \
    --out="$syntax_env/$method-pr.tsv" --tabbed="$syntax_env/$method.tsv"
done
```

To rerun the offline Stanford CoreNLP 4.6.0 OpenIE comparison on CaRB (using a local CoreNLP 4.6.0 distribution and Java 11+ with `java.desktop`):

```bash
python3 scripts/carb-predictions.py "$carb_checkout/data/test.txt" \
  --gold "$carb_checkout/data/gold/test.tsv" \
  --stanford-output "$carb_checkout/system_outputs/test/stanford_output.txt" \
  --corenlp-dir /path/to/stanford-corenlp-4.6.0 \
  --write-openie-evidence openie-evidence.jsonl

for view in raw anchored; do
  python3 scripts/carb-predictions.py "$carb_checkout/data/test.txt" \
    --corenlp-dir /path/to/stanford-corenlp-4.6.0 \
    --corenlp-view "$view" > "$syntax_env/corenlp-$view.tsv"
  "$syntax_env/bin/python" -c '
import pathlib, runpy, sys
import sklearn.preprocessing as preprocessing
sys.modules["sklearn.preprocessing.data"] = preprocessing
checkout = pathlib.Path(sys.argv.pop(1)).resolve()
sys.path.insert(0, str(checkout))
runpy.run_path(str(checkout / "carb.py"), run_name="__main__")
' "$carb_checkout" --gold="$carb_checkout/data/gold/test.tsv" \
    --out="$syntax_env/corenlp-$view.pr" --tabbed="$syntax_env/corenlp-$view.tsv"
done
```
