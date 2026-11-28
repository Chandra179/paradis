"""Evaluate local Stanford OpenIE proposals and verify their source spans."""

import json
import subprocess
from collections import Counter


MODEL = "stanford-corenlp-4.6.0"
CHUNK_LINES = 64
FIELDS = (("subject", "subject"), ("relation", "predicate"), ("object", "object"))


def utf16_length(text):
    return len(text.encode("utf-16-le")) // 2


def annotate(java, distribution, snapshot):
    if not (distribution / f"{MODEL}.jar").is_file() or not (distribution / f"{MODEL}-models.jar").is_file():
        raise ValueError(f"expected {MODEL} code and English models in {distribution}")
    lines = snapshot.read_text(encoding="utf-8").split("\n")
    chunk_dir = snapshot.parent / "corenlp-chunks"
    chunk_dir.mkdir(exist_ok=True)
    chunks = []
    for index in range(0, len(lines), CHUNK_LINES):
        chunk_text = "\n".join(lines[index:index + CHUNK_LINES])
        chunk_path = chunk_dir / f"chunk-{index:05d}.txt"
        chunk_path.write_text(chunk_text, encoding="utf-8")
        chunks.append((chunk_path, chunk_text))
    filelist = chunk_dir / "filelist.txt"
    filelist.write_text("\n".join(str(path) for path, _ in chunks), encoding="utf-8")
    result = subprocess.run(
        [
            java, "-Xmx2g", "-XX:ActiveProcessorCount=2", "-cp", str(distribution / "*"),
            "edu.stanford.nlp.pipeline.StanfordCoreNLP",
            "-annotators", "tokenize,ssplit,pos,lemma,depparse,natlog,openie",
            "-ssplit.eolonly", "true",
            "-tokenize.options", "invertible=true,ptb3Escaping=false,untokenizable=allKeep",
            "-openie.resolve_coref", "false",
            "-openie.triple.strict", "true",
            "-encoding", "UTF-8",
            "-outputFormat", "json",
            "-filelist", filelist,
            "-outputDirectory", chunk_dir,
        ],
        check=False, capture_output=True, encoding="utf-8", timeout=180,
    )
    if result.returncode:
        detail = next(iter(reversed(result.stderr.strip().splitlines())), "")
        raise ValueError(f"CoreNLP failed (exit {result.returncode}) {detail}")
    sentences = []
    utf16_offset = 0
    for chunk_path, chunk_text in chunks:
        output = chunk_path.with_name(f"{chunk_path.name}.json")
        if not output.is_file():
            output = chunk_path.with_suffix(".json")
        chunk_json = json.loads(output.read_text(encoding="utf-8"))
        for sentence in chunk_json["sentences"]:
            if utf16_offset:
                for token in sentence["tokens"]:
                    token["characterOffsetBegin"] += utf16_offset
                    token["characterOffsetEnd"] += utf16_offset
            sentences.append(sentence)
        utf16_offset += utf16_length(chunk_text) + 1
    return {"sentences": sentences}


def utf16_byte_offsets(text):
    offsets = [0]
    length = 0
    for character in text:
        if ord(character) > 0xFFFF:
            offsets.append(None)
        length += len(character.encode())
        offsets.append(length)
    return offsets


def token_offsets(tokens, encoded, offsets, line_start, line_end):
    ranges = []
    cursor = line_start
    for token in tokens:
        start_utf16, end_utf16 = token["characterOffsetBegin"], token["characterOffsetEnd"]
        if not 0 <= start_utf16 < end_utf16 < len(offsets):
            raise ValueError("CoreNLP returned an invalid UTF-16 token range")
        start, end = offsets[start_utf16], offsets[end_utf16]
        if (
            start is None or end is None
            or not cursor <= start < end <= line_end
            or encoded[cursor:start].decode().strip()
            or encoded[start:end] != token["originalText"].encode()
        ):
            raise ValueError("CoreNLP tokens do not align to the normalized source line")
        ranges.append((start, end))
        cursor = end
    if not ranges or encoded[cursor:line_end].decode().strip():
        raise ValueError("CoreNLP did not cover the normalized source line")
    return ranges


def evidence(document_id, encoded, start, end):
    return {
        "document_version_id": document_id,
        "start_byte": start,
        "end_byte": end,
        "quote": encoded[start:end].decode(),
    }


def anchor_arguments(triple, tokens, ranges, document_id, encoded):
    arguments = {}
    for field, label in FIELDS:
        start, end = triple[f"{field}Span"]
        if not 0 <= start < end <= len(tokens):
            return None, "invalid_token_span"
        # CoreNLP reports one bounding interval, even for synthesized or
        # noncontiguous fields. Require all original tokens to support the gloss.
        words = " ".join(token["originalText"] for token in tokens[start:end])
        if words.split() != triple[field].split():
            return None, "nonliteral_argument"
        arguments[label] = evidence(document_id, encoded, ranges[start][0], ranges[end - 1][1])
    ordered = sorted(arguments.values(), key=lambda item: item["start_byte"])
    if any(left["end_byte"] > right["start_byte"] for left, right in zip(ordered, ordered[1:])):
        return None, "overlapping_arguments"
    return arguments, None


def compare(document, annotations):
    text = document["normalized_text"]
    encoded = text.encode()
    offsets = utf16_byte_offsets(text)
    sentences = annotations["sentences"]
    lines = text.split("\n")
    if len(lines) != len(sentences):
        raise ValueError(f"expected {len(lines)} CoreNLP sentence annotations, got {len(sentences)}")

    raw, anchored, candidates = [], [], []
    withheld = Counter()
    line_start = 0
    for line, sentence in zip(lines, sentences):
        line_end = line_start + len(line.encode())
        tokens = sentence["tokens"]
        ranges = token_offsets(tokens, encoded, offsets, line_start, line_end)
        for proposal in sentence.get("openie", ()):
            # The native JSON format does not carry CoreNLP confidence scores.
            raw.append((line, "1", proposal["relation"], proposal["subject"], proposal["object"]))
            arguments, reason = anchor_arguments(proposal, tokens, ranges, document["id"], encoded)
            if arguments is None:
                withheld[reason] += 1
                continue
            anchored.append(raw[-1])
            candidates.append({
                "record_type": "candidate",
                "candidate": {
                    "extractor_version": "corenlp-openie-adapter-v1",
                    "rule_id": "source_anchored_model_proposal",
                    "openie_model": MODEL,
                    "value": {"kind": "relationship", **{label: proposal[field] for field, label in FIELDS}},
                    "evidence": evidence(document["id"], encoded, line_start, line_end),
                    "argument_evidence": arguments,
                },
            })
        line_start = line_end + 1
    return raw, anchored, candidates, dict(withheld)
