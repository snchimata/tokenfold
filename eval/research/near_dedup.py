#!/usr/bin/env python3
"""Report lexical near duplicates in JSONL without modifying the corpus."""

import argparse
import json
import re
from pathlib import Path

WORDS = re.compile(r"[A-Za-z0-9_]+")


def tokens(value) -> set[str]:
    return {word.lower() for word in WORDS.findall(json.dumps(value, sort_keys=True))}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("jsonl", type=Path)
    parser.add_argument("--threshold", type=float, default=0.9)
    args = parser.parse_args(argv)
    if not 0 <= args.threshold <= 1:
        parser.error("threshold must be in [0,1]")
    rows = [json.loads(line) for line in args.jsonl.read_text(encoding="utf-8").splitlines() if line]
    token_sets = [tokens(row) for row in rows]
    matches = []
    for left in range(len(rows)):
        for right in range(left + 1, len(rows)):
            union = token_sets[left] | token_sets[right]
            score = len(token_sets[left] & token_sets[right]) / len(union) if union else 1.0
            if score >= args.threshold:
                matches.append({"left": left + 1, "right": right + 1, "jaccard": score})
    print(json.dumps({"rows": len(rows), "threshold": args.threshold, "matches": matches}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
