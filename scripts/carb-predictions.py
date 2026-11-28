#!/usr/bin/env python3
"""Export evidence-linked relationships in CaRB's tab-separated prediction format."""

import argparse
import bisect
import hashlib
import json
import string
import subprocess
import tempfile
from collections import Counter
from pathlib import Path

import corenlp_openie


# CaRB uses these Penn Treebank escapes and strips punctuation for sentence keys.
BRACKET_ESCAPES = ("-LRB-", "-RRB-", "-LSB-", "-RSB-", "-LCB-", "-RCB-")
SENTENCE_PUNCTUATION = str.maketrans("", "", string.punctuation + " ")


def sentence_key(sentence):
    for escape in BRACKET_ESCAPES:
        sentence = sentence.replace(escape, "")
    return sentence.translate(SENTENCE_PUNCTUATION)


def stanford_predictions(path):
    with path.open(encoding="utf-8") as source:
        for line in source:
            if not line.strip():
                continue
            fields = line.rstrip("\r\n").split("\t")
            if len(fields) < 13:
                raise ValueError("unexpected Stanford prediction row")
            subject, predicate, obj = fields[2:5]
            yield fields[12], fields[11], predicate, subject, obj


def strict_counts(exported, gold, sentences):
    counts = Counter()
    unmapped = 0
    for sentence, _, *fields in exported:
        key = sentence_key(sentence)
        if key in sentences:
            counts[(key, *(field.strip() for field in fields))] += 1
        else:
            unmapped += 1
    return {
        "sentences_with_predictions": len({fields[0] for fields in counts}),
        "predicted_tuples": counts.total(),
        "unmapped_sentence_rows": unmapped,
        "exact_matches": (counts & gold).total(),
    }


def input_lines(sentences):
    lines = list(dict.fromkeys(line.strip() for line in sentences if line.strip()))
    if not lines:
        raise ValueError("expected at least one sentence")
    return lines


def extraction_rows(executable, text, options):
    result = subprocess.run(
        [executable, *options], check=True, capture_output=True, encoding="utf-8"
    )
    rows = [json.loads(line) for line in result.stdout.splitlines()]
    if len(rows) < 2 or rows[1]["record_type"] != "document":
        raise ValueError("the extractor did not accept the sentence file")
    document = rows[1]["document"]
    digest = hashlib.sha256(text.encode()).hexdigest()
    if (
        document["normalized_text"] != text
        or document["normalized_text_sha256"] != digest
        or document["payload_sha256"] != digest
    ):
        raise ValueError("the extractor changed the benchmark text or its digest")
    return rows


def predictions(executable, sentences, dependency_python=None):
    lines = input_lines(sentences)
    text = "\n".join(lines)
    encoded = text.encode()
    starts = [0]
    for line in lines:
        starts.append(starts[-1] + len(line.encode()) + 1)

    with tempfile.TemporaryDirectory() as directory:
        snapshot = Path(directory) / "carb.txt"
        snapshot.write_bytes(encoded)
        options = []
        if dependency_python is not None:
            annotations = Path(directory) / "dependencies.json"
            adapter = Path(__file__).with_name("parse-dependencies.py")
            with annotations.open("wb") as output:
                subprocess.run(
                    [dependency_python, adapter, "--one-sentence-per-line", snapshot],
                    check=True,
                    stdout=output,
                )
            options = ["--dependencies", annotations]
        rows = extraction_rows(executable, text, [*options, "--", snapshot])
    document = rows[1]["document"]
    if rows[1]["truncated"]:
        raise ValueError("the extractor truncated the benchmark candidates")

    for row in rows[2:]:
        candidate = row["candidate"]
        value = candidate["value"]
        if value["kind"] != "relationship":
            continue
        evidence = candidate["evidence"]
        start, end = evidence["start_byte"], evidence["end_byte"]
        index = bisect.bisect_right(starts, start) - 1
        if (
            evidence["document_version_id"] != document["id"]
            or not 0 <= index < len(lines)
            or not 0 <= start < end <= starts[index + 1] - 1
            or encoded[start:end] != evidence["quote"].encode()
        ):
            raise ValueError("a relationship has invalid evidence or spans multiple sentences")
        # This baseline supplies no calibrated confidence, so all tuples have one score.
        yield lines[index], "1", value["predicate"], value["subject"], value["object"]


def openie_predictions(executable, sentences, source_id, java, corenlp):
    text = "\n".join(input_lines(sentences))
    with tempfile.TemporaryDirectory() as directory:
        snapshot = Path(directory) / "corpus.txt"
        snapshot.write_bytes(text.encode())
        manifest = snapshot.with_suffix(".manifest.json")
        manifest.write_text(json.dumps({
            "schema_version": 1,
            "snapshots": [{"id": "corpus", "source_id": source_id, "path": snapshot.name}],
        }), encoding="utf-8")
        document = extraction_rows(executable, text, ["--snapshot-manifest", manifest])[1]["document"]
        annotations = corenlp_openie.annotate(java, corenlp, snapshot)
        return document, corenlp_openie.compare(document, annotations)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sentences", type=Path)
    parser.add_argument("--extractor", type=Path, default=Path("target/debug/paradis-extract"))
    parser.add_argument("--gold", type=Path, help="report strict tuple counts instead of TSV")
    parser.add_argument("--stanford-output", type=Path, help="score the published Stanford predictions alongside the local extractor")
    parser.add_argument("--dependency-python", type=Path, help="Python executable with spaCy and en_core_web_sm for the offline comparison")
    parser.add_argument("--corenlp-dir", type=Path, help="local Stanford CoreNLP 4.6.0 distribution")
    parser.add_argument("--java", default="java", help="Java 11+ executable")
    parser.add_argument("--corenlp-view", choices=("raw", "anchored"), help="choose which CoreNLP TSV to export")
    parser.add_argument("--write-openie-evidence", type=Path, help="write normalized text and fully anchored CoreNLP candidates as JSON Lines")
    args = parser.parse_args()
    if args.stanford_output is not None and args.gold is None:
        parser.error("--stanford-output requires --gold")
    if (args.corenlp_view or args.write_openie_evidence) and args.corenlp_dir is None:
        parser.error("the CoreNLP export options require --corenlp-dir")
    if args.corenlp_view and args.gold is not None:
        parser.error("--corenlp-view selects TSV output and requires omitting --gold")
    try:
        lines = args.sentences.read_text(encoding="utf-8").splitlines()
        executable = args.extractor.resolve()
        corenlp = None
        if args.corenlp_dir is not None:
            source_id = f"openie-corpus:{args.sentences.resolve()}"
            document, corenlp = openie_predictions(executable, lines, source_id, args.java, args.corenlp_dir.resolve())
            if args.write_openie_evidence is not None:
                with args.write_openie_evidence.open("w", encoding="utf-8") as output:
                    for row in [{"record_type": "document", "document": document}, *corenlp[2]]:
                        print(json.dumps(row, ensure_ascii=False), file=output)
        if args.gold is None:
            exported = (
                corenlp[0 if args.corenlp_view == "raw" else 1] if corenlp is not None
                else predictions(executable, lines, args.dependency_python)
            )
            for fields in exported:
                print("\t".join(fields))
        else:
            raw_gold = (
                tuple(field.strip() for field in line.split("\t"))
                for line in args.gold.read_text(encoding="utf-8").splitlines()
                if line.strip()
            )
            gold = Counter((sentence_key(fields[0]), *fields[1:]) for fields in raw_gold)
            unique = {line.strip() for line in lines if line.strip()}
            keys = {sentence_key(line) for line in unique}
            if len(keys) != len(unique) or any(fields[0] not in keys for fields in gold):
                raise ValueError("sentence keys are ambiguous or gold data does not match the input split")
            report = {
                "sentences": len(keys),
                "gold_tuples": gold.total(),
                "local": strict_counts(predictions(executable, lines), gold, keys),
            }
            if args.dependency_python is not None:
                dependent = predictions(executable, lines, args.dependency_python)
                report["dependency"] = strict_counts(dependent, gold, keys)
            if args.stanford_output is not None:
                report["stanford"] = strict_counts(stanford_predictions(args.stanford_output), gold, keys)
            if corenlp is not None:
                raw, anchored, _, withheld = corenlp
                report["corenlp_model"] = corenlp_openie.MODEL
                report["corenlp_raw"] = strict_counts(raw, gold, keys)
                report["corenlp_anchored"] = strict_counts(anchored, gold, keys)
                report["corenlp_withheld_arguments"] = withheld
            print(json.dumps(report, indent=2))
    except (KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"CaRB export failed: {error}\n")


if __name__ == "__main__":
    main()
