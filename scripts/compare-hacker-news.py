#!/usr/bin/env python3
"""Compare two title-only Hacker News JSON Lines captures by story ID."""

import argparse
import hashlib
import json
from collections import Counter
from pathlib import Path


def read_capture(path, normalizer):
    documents = {}
    versions = {}
    attempts = []
    with path.open(encoding="utf-8") as capture:
        for line in capture:
            row = json.loads(line)
            record_type = row["record_type"]
            if record_type == "request_attempt":
                attempts.append({
                    key: row.get(key)
                    for key in ("request_kind", "http_status", "disposition", "reason")
                })
                continue
            if record_type == "document":
                document = row["document"]
                story_id = row["source_record"]["id"]
                version = document["id"]
                text = document["normalized_text"].encode()
                if (
                    story_id in documents
                    or version in versions
                    or document["normalizer_version"] != normalizer
                    or hashlib.sha256(text).hexdigest() != document["normalized_text_sha256"]
                ):
                    raise ValueError(f"{path}: inconsistent document for story {story_id}")
                documents[story_id] = (text, Counter())
                versions[version] = story_id
                continue
            if record_type == "candidate":
                candidate = row["candidate"]
                evidence = candidate["evidence"]
                version = evidence["document_version_id"]
                if version not in versions:
                    raise ValueError(f"{path}: candidate cites an unknown version ID")
                text, candidates = documents[versions[version]]
                start, end = evidence["start_byte"], evidence["end_byte"]
                quote = evidence["quote"]
                if (
                    not 0 <= start < end <= len(text)
                    or text[start:end] != quote.encode()
                    or not candidate["rule_id"]
                    or not candidate["extractor_version"]
                ):
                    raise ValueError(f"{path}: candidate cites an invalid evidence span")
                candidates[(json.dumps(candidate["value"], sort_keys=True), start, end, quote)] += 1
                continue
            raise ValueError(f"{path}: unexpected record type {record_type!r}")
    if not attempts:
        raise ValueError(f"{path}: missing request attempts")
    return documents, attempts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("api_capture", type=Path)
    parser.add_argument("other_capture", type=Path)
    parser.add_argument("--route", choices=("page", "rss"), default="page")
    args = parser.parse_args()
    try:
        api, api_attempts = read_capture(args.api_capture, "hn-title-v1")
        route = args.route
        normalizer = {"page": "hn-front-page-title-v1", "rss": "hn-rss-title-v1"}[route]
        other, other_attempts = read_capture(args.other_capture, normalizer)
    except (KeyError, OSError, ValueError) as error:
        parser.exit(1, f"Hacker News comparison failed: {error}\n")
    shared = api.keys() & other.keys()
    matching_title = {story_id for story_id in shared if api[story_id][0] == other[story_id][0]}
    print(json.dumps({
        "api_titles": len(api),
        f"{route}_titles": len(other),
        "matched_story_ids": len(shared),
        "exact_title_matches": len(matching_title),
        "matching_candidate_sets_on_equal_titles": sum(
            api[story_id][1] == other[story_id][1] for story_id in matching_title
        ),
        "api_candidates": sum(candidates.total() for _, candidates in api.values()),
        f"{route}_candidates": sum(candidates.total() for _, candidates in other.values()),
        "api_attempts": api_attempts,
        f"{route}_attempts": other_attempts,
    }, indent=2))


if __name__ == "__main__":
    main()
