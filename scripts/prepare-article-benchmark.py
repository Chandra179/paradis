#!/usr/bin/env python3
"""Prepare the Zyte article benchmark's saved HTML and body labels for local scoring."""

import argparse
import gzip
import json
from pathlib import Path


MAX_INPUT_BYTES = 2 * 1024 * 1024


def prepare(source, destination, limit):
    labels = json.loads((source / "ground-truth.json").read_text(encoding="utf-8"))
    ids = sorted(labels)
    if limit is not None:
        ids = ids[:limit]
    destination.mkdir()
    fixtures = []
    for index, item_id in enumerate(ids, 1):
        if Path(item_id).name != item_id:
            raise ValueError(f"unexpected article ID {item_id!r}")
        with gzip.open(source / "html" / f"{item_id}.html.gz", "rb") as archive:
            raw = archive.read(MAX_INPUT_BYTES + 1)
        filename = f"article-{index:04d}.html"
        (destination / filename).write_bytes(raw)
        fixtures.append({
            "path": filename,
            "source_type": "zyte_article",
            "source_item_id": item_id,
            "reference_text": labels[item_id]["articleBody"],
        })

    manifest = destination / "corpus.json"
    manifest.write_text(json.dumps({
        "schema_version": 1,
        "name": "zyte-article-extraction",
        "fixtures": fixtures,
    }, ensure_ascii=False), encoding="utf-8")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    if args.limit is not None and args.limit < 1:
        parser.error("--limit must be positive")
    try:
        print(prepare(args.checkout, args.destination, args.limit))
    except (KeyError, OSError, ValueError) as error:
        parser.exit(1, f"article preparation failed: {error}\n")


if __name__ == "__main__":
    main()
