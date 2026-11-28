#!/usr/bin/env python3
"""Annotate exact normalized text with the local spaCy English pipeline."""

import argparse
import hashlib
import json
from pathlib import Path


def annotate(path, presegmented):
    import spacy

    text = path.read_bytes().decode("utf-8")
    parser = spacy.load("en_core_web_sm", exclude=["ner", "lemmatizer"])
    offsets = [0]
    for character in text:
        offsets.append(offsets[-1] + len(character.encode()))

    def inputs():
        position = 0
        for line in text.splitlines(keepends=True):
            if line.strip():
                doc = parser.make_doc(line.rstrip("\r\n"))
                for token in doc:
                    token.is_sent_start = token.i == 0
                yield doc, position
            position += len(line)

    docs = parser.pipe(inputs(), as_tuples=True) if presegmented else [(parser(text), 0)]
    sentences = []
    for doc, position in docs:
        for sentence in doc.sents:
            tokens = [token for token in sentence if not token.is_space]
            indices = {token.i: index for index, token in enumerate(tokens)}
            if tokens:
                sentences.append([
                    {
                        "start_byte": offsets[position + token.idx],
                        "end_byte": offsets[position + token.idx + len(token.text)],
                        "head": None if token.head.i == token.i else indices[token.head.i],
                        "pos": token.pos_,
                        "relation": token.dep_,
                    }
                    for token in tokens
                ])
    return {
        "parser": f"spacy/{spacy.__version__} en_core_web_sm/{parser.meta['version']}",
        "normalized_text_sha256": hashlib.sha256(text.encode()).hexdigest(),
        "sentences": sentences,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("normalized_text", type=Path)
    parser.add_argument("--one-sentence-per-line", action="store_true")
    args = parser.parse_args()
    try:
        annotations = annotate(args.normalized_text, args.one_sentence_per_line)
    except (ImportError, KeyError, OSError, ValueError) as error:
        parser.exit(1, f"dependency parsing failed: {error}\n")
    print(json.dumps(annotations, separators=(",", ":")))


if __name__ == "__main__":
    main()
