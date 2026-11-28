#!/usr/bin/env python3
"""Score saved-document CLI output against independently annotated fixtures."""

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
from collections import Counter
from pathlib import Path
from typing import NamedTuple


KINDS = ("date", "quantity", "relationship")


class Observation(NamedTuple):
    attempt: dict
    text: str
    candidates: list
    invalid_evidence: int
    elapsed_ns: int
    peak_rss_kib: int | None = None

    @property
    def accepted(self):
        return self.attempt["disposition"] == "accepted"


def run_process(command, measure_rss=False, timeout=15):
    if not measure_rss:
        return subprocess.run(
            command, check=True, capture_output=True, encoding="utf-8", timeout=timeout
        ), None
    with tempfile.NamedTemporaryFile() as meter:
        measured = ["/usr/bin/time", "-q", "-f", "%M", "-o", meter.name, *command]
        with subprocess.Popen(
            measured, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            encoding="utf-8", start_new_session=True
        ) as child:
            try:
                stdout, stderr = child.communicate(timeout=timeout)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.communicate()
                raise
        result = subprocess.CompletedProcess(command, child.returncode, stdout, stderr)
        result.check_returncode()
        return result, int(meter.read())


def extract(executable, path, options=(), measure_rss=False):
    started = time.perf_counter_ns()
    result, peak = run_process([executable, *options, "--", path], measure_rss)
    elapsed_ns = time.perf_counter_ns() - started
    rows = [json.loads(line) for line in result.stdout.splitlines()]
    if not rows or rows[0]["record_type"] != "attempt" or rows[0]["attempt_index"] != 1:
        raise ValueError(f"{path}: expected one attempt record")
    attempt = rows[0]
    if attempt["disposition"] in ("skipped", "uncertain"):
        if len(rows) != 1 or not attempt["reason"] or attempt["document_version_id"] is not None:
            raise ValueError(f"{path}: withheld attempt lacks a reason or has document output")
        return Observation(attempt, "", [], 0, elapsed_ns, peak)
    if attempt["disposition"] != "accepted" or len(rows) < 2:
        raise ValueError(f"{path}: unexpected attempt disposition or missing document")

    document_row = rows[1]
    if document_row["record_type"] != "document" or document_row["attempt_index"] != 1:
        raise ValueError(f"{path}: expected one document record")
    document = document_row["document"]
    text = document["normalized_text"]
    encoded = text.encode()
    if (
        document["id"] != attempt["document_version_id"]
        or hashlib.sha256(encoded).hexdigest() != document["normalized_text_sha256"]
        or hashlib.sha256(path.read_bytes()).hexdigest() != document["payload_sha256"]
        or document_row["candidate_count"] != len(rows) - 2
    ):
        raise ValueError(f"{path}: inconsistent document, digest, or candidate count")

    candidates = []
    invalid = 0
    for row in rows[2:]:
        if row["record_type"] != "candidate" or row["attempt_index"] != 1:
            raise ValueError(f"{path}: expected a candidate for attempt 1")
        candidate = row["candidate"]
        evidence = candidate["evidence"]
        start, end = evidence["start_byte"], evidence["end_byte"]
        quote = evidence["quote"]
        valid = (
            evidence["document_version_id"] == document["id"]
            and 0 <= start < end <= len(encoded)
            and encoded[start:end] == quote.encode()
            and candidate["rule_id"]
            and candidate["extractor_version"]
        )
        invalid += not valid
        # Invalid evidence can incur a false positive but can never match a label.
        candidates.append((candidate["value"], quote if valid else None))
    return Observation(attempt, text, candidates, invalid, elapsed_ns, peak)


def score(predicted, expected):
    overlap = sum((predicted & expected).values())
    return Counter(tp=overlap, fp=predicted.total() - overlap, fn=expected.total() - overlap)


def key(value, quote):
    if value["kind"] not in KINDS:
        raise ValueError(f"unknown candidate type {value['kind']!r}")
    return value["kind"], json.dumps(value, sort_keys=True), quote


def candidate_scores(predicted, expected):
    predicted = Counter(key(value, quote) for value, quote in predicted)
    expected = Counter(key(value, quote) for value, quote in expected)
    return {
        kind: score(
            Counter({item: count for item, count in predicted.items() if item[0] == kind}),
            Counter({item: count for item, count in expected.items() if item[0] == kind}),
        )
        for kind in KINDS
    }


def metrics(counts):
    tp, fp, fn = (counts[key] for key in ("tp", "fp", "fn"))
    return {
        "tp": tp,
        "fp": fp,
        "fn": fn,
        "precision": tp / (tp + fp) if tp + fp else None,
        "recall": tp / (tp + fn) if tp + fn else None,
        "f1": 2 * tp / (2 * tp + fp + fn) if tp + fp + fn else None,
    }


def tokens(text):
    return Counter(re.findall(r"\w+", text.casefold()))


def run_readability(executable, command, measure_rss):
    started = time.perf_counter_ns()
    try:
        result, selector_peak = run_process(command, measure_rss)
    except subprocess.TimeoutExpired:
        return Observation({"disposition": "skipped"}, "", [], 0, time.perf_counter_ns() - started), None, "timeout"
    except subprocess.CalledProcessError:
        return Observation({"disposition": "skipped"}, "", [], 0, time.perf_counter_ns() - started), None, "selector_process_failed"
    adapter_elapsed = time.perf_counter_ns() - started
    parsed = json.loads(result.stdout)
    # Render the selected HTML with the same renderer and extractor as the controls.
    with tempfile.TemporaryDirectory() as directory:
        selected = Path(directory) / "readability.html"
        selected.write_text(parsed["content"], encoding="utf-8")
        observed = extract(executable, selected, ("--html-body",), measure_rss)
    observed = observed._replace(
        elapsed_ns=observed.elapsed_ns + adapter_elapsed,
        peak_rss_kib=max(selector_peak, observed.peak_rss_kib) if measure_rss else None,
    )
    return observed, parsed["packages"], parsed["error"]


def run_dependencies(executable, source, text, directory, python):
    adapter = Path(__file__).with_name("parse-dependencies.py")
    normalized = Path(directory) / "normalized.txt"
    normalized.write_bytes(text.encode())
    annotations = Path(directory) / "dependencies.json"
    started = time.perf_counter_ns()
    with annotations.open("wb") as output:
        subprocess.run([python, adapter, normalized], check=True, stdout=output, timeout=30)
    observed = extract(executable, source, ("--dependencies", annotations))
    return observed.candidates, observed.invalid_evidence, time.perf_counter_ns() - started


def record_html_comparison(group, methods, expected, reference):
    comparison = group.setdefault("html_comparison", {"fixtures": 0})
    comparison["fixtures"] += 1
    reference_tokens = tokens(reference)
    for name, observed in methods.items():
        scores = comparison.setdefault(name, {
            "main_text": Counter(),
            "label_quotes_retained": 0,
            "pipeline_elapsed_ns": 0,
            "accepted": 0,
            "peak_rss_kib": None,
            "invalid_evidence": 0,
            "candidates": {kind: Counter() for kind in KINDS},
        })
        scores["main_text"].update(score(tokens(observed.text), reference_tokens))
        scores["label_quotes_retained"] += sum(quote in observed.text for _, quote in (expected or ()))
        scores["pipeline_elapsed_ns"] += observed.elapsed_ns
        scores["accepted"] += int(observed.accepted)
        if observed.peak_rss_kib is not None:
            scores["peak_rss_kib"] = max(scores["peak_rss_kib"] or 0, observed.peak_rss_kib)
        scores["invalid_evidence"] += observed.invalid_evidence
        if expected is not None:
            for kind, counts in candidate_scores(observed.candidates, expected).items():
                scores["candidates"][kind].update(counts)


def benchmark(executable, manifest, readability_packages=None, dependency_python=None, *,
              rust_baseline=None, measure_rss=False):
    corpus = json.loads(manifest.read_text(encoding="utf-8"))
    if corpus["schema_version"] != 1 or not corpus["fixtures"]:
        raise ValueError("expected a nonempty corpus with schema_version 1")

    totals = {}
    packages = None
    readability_errors = []
    rust_packages = {}
    rust_errors = []
    screening = []
    for fixture in corpus["fixtures"]:
        inline = fixture.get("snapshot_text")
        reference = fixture.get("reference_text", inline)
        source_name = fixture.get("path", fixture.get("source_item_id", fixture.get("id", "inline fixture")))
        if not isinstance(reference, str) or ("path" in fixture) == (inline is not None):
            raise ValueError(f"{source_name}: expected a reference and one input source")
        labels = fixture.get("candidates")
        expected = [(label["value"], label["quote"]) for label in labels] if labels is not None else None
        if any(not quote or quote not in reference for _, quote in (expected or ())):
            raise ValueError(f"{source_name}: label quote is absent from reference text")

        with tempfile.TemporaryDirectory() as directory:
            if inline is None:
                source = (manifest.parent / fixture["path"]).resolve()
            else:
                suffix = fixture.get("snapshot_format", corpus.get("inline_format", "txt"))
                if suffix not in ("html", "txt"):
                    raise ValueError(f"{source_name}: unsupported inline snapshot format")
                source = Path(directory) / f"snapshot.{suffix}"
                source.write_text(inline, encoding="utf-8")
            is_html = source.suffix.lower() in (".html", ".htm")
            observed = extract(executable, source, measure_rss=measure_rss and is_html)
            accepted, text = observed.accepted, observed.text
            predicted, invalid = observed.candidates, observed.invalid_evidence
            syntax = ([], 0, 0)
            if accepted and dependency_python is not None:
                syntax = run_dependencies(executable, source, text, directory, dependency_python)
                if [item for item in syntax[0] if item[0]["kind"] != "relationship"] != [
                    item for item in predicted if item[0]["kind"] != "relationship"
                ]:
                    raise ValueError(f"{source_name}: dependency mode changed a date or quantity")
            methods = {}
            if is_html:
                methods["pruned"] = observed
                methods["body"] = extract(executable, source, ("--html-body",), measure_rss)
                if readability_packages is not None:
                    adapter = Path(__file__).with_name("readability-baseline.mjs")
                    methods["readability"], versions, error = run_readability(
                        executable, ["node", adapter, readability_packages, source], measure_rss
                    )
                    packages = versions or packages
                    if error:
                        readability_errors.append({
                            "fixture": fixture.get("source_item_id", source_name),
                            "error": error,
                        })
                if rust_baseline is not None:
                    for method in ("dom_smoothie", "legible"):
                        methods[method], versions, error = run_readability(
                            executable, [rust_baseline, method, source], measure_rss
                        )
                        if versions is not None:
                            rust_packages[method] = versions["version"]
                        if error:
                            rust_errors.append({"fixture": source_name, "method": method, "error": error})
            reference_file = Path(directory) / "reference.txt"
            reference_file.write_text(reference, encoding="utf-8")
            reference_run = extract(executable, reference_file)
            reference_predictions = reference_run.candidates
            reference_invalid = reference_run.invalid_evidence

        expected_disposition = fixture.get("expected_disposition")
        if expected_disposition is not None:
            if expected_disposition not in ("accepted", "skipped", "uncertain"):
                raise ValueError(f"{source_name}: unknown expected disposition")
            screening.append({
                "fixture": source_name,
                "expected_disposition": expected_disposition,
                "expected_reason": fixture.get("expected_reason"),
                "disposition": observed.attempt["disposition"],
                "reason": observed.attempt["reason"],
                "page_screening": observed.attempt.get("page_screening"),
                "reference_text_retained": reference in text if reference else None,
            })

        if expected is not None:
            observed_scores = candidate_scores(predicted, expected)
            reference_scores = candidate_scores(reference_predictions, expected)
        text_score = score(tokens(text), tokens(reference))
        useful = bool(reference.strip())
        for name in ("overall", f"source/{fixture['source_type']}"):
            group = totals.setdefault(name, {
                "inputs": Counter(),
                "main_text": Counter(),
                "pipeline": {kind: Counter() for kind in KINDS},
                "reference_text": {kind: Counter() for kind in KINDS},
                "evidence": Counter(),
            })
            group["inputs"].update({
                "fixtures": 1,
                "useful": int(useful),
                "useful_accepted": int(useful and accepted),
                "unusable": int(not useful),
                "unusable_skipped": int(not useful and observed.attempt["disposition"] == "skipped"),
                "unusable_withheld": int(not useful and not accepted),
                "skipped": int(observed.attempt["disposition"] == "skipped"),
                "uncertain": int(observed.attempt["disposition"] == "uncertain"),
                "candidate_labeled_fixtures": int(expected is not None),
            })
            group["main_text"].update(text_score)
            group["evidence"].update({
                "labels": len(expected or ()),
                "label_quotes_retained": sum(quote in text for _, quote in (expected or ())),
                "pipeline_candidates": len(predicted),
                "pipeline_invalid": invalid,
                "reference_candidates": len(reference_predictions),
                "reference_invalid": reference_invalid,
            })
            if expected is not None:
                for kind in KINDS:
                    group["pipeline"][kind].update(observed_scores[kind])
                    group["reference_text"][kind].update(reference_scores[kind])
            if dependency_python is not None:
                syntax_candidates, syntax_invalid, syntax_elapsed = syntax
                comparison = group.setdefault("dependency_relationships", {
                    "scores": Counter(),
                    "invalid_evidence": 0,
                    "pipeline_elapsed_ns": 0,
                })
                comparison["invalid_evidence"] += syntax_invalid
                comparison["pipeline_elapsed_ns"] += syntax_elapsed
                if expected is not None:
                    comparison["scores"].update(candidate_scores(syntax_candidates, expected)["relationship"])
            if methods:
                record_html_comparison(group, methods, expected, reference)

    for group in totals.values():
        inputs = group["inputs"]
        withheld = inputs["skipped"] + inputs["uncertain"]
        group["inputs"] = {
            **inputs,
            "useful_retention": inputs["useful_accepted"] / inputs["useful"] if inputs["useful"] else None,
            "unusable_skip_precision": inputs["unusable_skipped"] / inputs["skipped"] if inputs["skipped"] else None,
            "unusable_withhold_precision": inputs["unusable_withheld"] / withheld if withheld else None,
            "unusable_withhold_recall": inputs["unusable_withheld"] / inputs["unusable"] if inputs["unusable"] else None,
        }
        group["main_text"] = metrics(group["main_text"])
        for stage in ("pipeline", "reference_text"):
            group[stage] = {kind: metrics(counts) for kind, counts in group[stage].items()}
        if comparison := group.get("dependency_relationships"):
            comparison["scores"] = metrics(comparison["scores"])
        if comparison := group.get("html_comparison"):
            for method, scores in comparison.items():
                if method != "fixtures":
                    scores["main_text"] = metrics(scores["main_text"])
                    scores["candidates"] = {
                        kind: metrics(counts) for kind, counts in scores["candidates"].items()
                    }
    report = {"corpus": corpus["name"], "groups": totals}
    if packages is not None:
        report["readability_packages"] = packages
    if readability_packages is not None:
        report["readability_errors"] = readability_errors
    if rust_baseline is not None:
        report["rust_readability_packages"] = rust_packages
        report["rust_readability_errors"] = rust_errors
    if screening:
        report["page_screening"] = {
            "labeled_fixtures": len(screening),
            "exact_disposition_matches": sum(row["disposition"] == row["expected_disposition"] for row in screening),
            "exact_disposition_and_reason_matches": sum(
                row["disposition"] == row["expected_disposition"] and row["reason"] == row["expected_reason"]
                for row in screening
            ),
            "fixtures": screening,
        }
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--extractor", type=Path, default=Path("target/debug/paradis-extract"))
    parser.add_argument("--readability-node-modules", type=Path)
    parser.add_argument("--rust-readability-baseline", type=Path)
    parser.add_argument("--measure-peak-rss", action="store_true", help="measure HTML comparison processes with GNU time")
    parser.add_argument("--dependency-python", type=Path, help="Python executable with spaCy and en_core_web_sm for the offline comparison")
    args = parser.parse_args()
    try:
        packages = args.readability_node_modules
        if packages is not None and (not packages.is_dir() or shutil.which("node") is None):
            raise ValueError("Mozilla Readability requires Node and a node_modules directory")
        rust_baseline = args.rust_readability_baseline
        if rust_baseline is not None and (not rust_baseline.is_file() or not os.access(rust_baseline, os.X_OK)):
            raise ValueError("Rust Readability comparator must be an executable file")
        report = benchmark(
            args.extractor.resolve(),
            args.manifest.resolve(),
            packages.resolve() if packages else None,
            args.dependency_python,
            rust_baseline=rust_baseline.resolve() if rust_baseline else None,
            measure_rss=args.measure_peak_rss,
        )
    except (KeyError, OSError, TypeError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"benchmark failed: {error}\n")
    print(json.dumps(report, indent=2, sort_keys=True))
    overall = report["groups"]["overall"]
    evidence = overall["evidence"]
    comparisons = overall.get("html_comparison", {})
    invalid = sum(scores["invalid_evidence"] for name, scores in comparisons.items() if name != "fixtures")
    invalid += overall.get("dependency_relationships", {}).get("invalid_evidence", 0)
    return int(bool(evidence["pipeline_invalid"] or evidence["reference_invalid"] or invalid))


if __name__ == "__main__":
    raise SystemExit(main())
