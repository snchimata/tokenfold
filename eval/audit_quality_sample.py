#!/usr/bin/env python3
"""Create/check a deterministic one-fixture-per-family human quality audit."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def select(tasks_dir: Path) -> list[tuple[Path, dict]]:
    by_family: dict[str, list[tuple[Path, dict]]] = {}
    for path in sorted(tasks_dir.glob("*.json")):
        fixture = json.loads(path.read_text(encoding="utf-8"))
        by_family.setdefault(fixture["family"], []).append((path, fixture))
    return [
        min(items, key=lambda item: hashlib.sha256(item[1]["id"].encode()).digest())
        for _family, items in sorted(by_family.items())
    ]


def render(sample: list[tuple[Path, dict]]) -> str:
    lines = [
        "# v0.4 quality audit sample",
        "",
        "Status: **pending human review**",
        "Reviewer: ",
        "Reviewed at (UTC): ",
        "",
        "Check each item against its full JSON fixture. Do not mark an item complete unless the",
        "question has one unambiguous answer, the evidence span supports that answer, critical",
        "atoms are genuinely safety-relevant, and the synthetic provenance note is credible.",
        "",
    ]
    for path, fixture in sample:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        lines.extend(
            [
                f"## [ ] `{fixture['id']}` ({fixture['family']})",
                "",
                f"- Fixture SHA-256: `{digest}`",
                f"- Query: {fixture['query']}",
                f"- Expected answer: `{fixture['gold_answer']}`",
                "- [ ] Answer is unique and unambiguous.",
                "- [ ] Supporting evidence entails the expected answer.",
                "- [ ] Critical atoms are appropriate and separate from answer evidence.",
                "- [ ] Provenance contains no private, third-party, or secret material.",
                "- Reviewer notes: ",
                "",
            ]
        )
    return "\n".join(lines)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tasks-dir", type=Path, default=Path(__file__).parent / "tasks/v04")
    parser.add_argument("--output", type=Path, default=Path(__file__).parent / "tasks/v04/HUMAN_AUDIT.md")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    if args.check:
        text = args.output.read_text(encoding="utf-8")
        sample = select(args.tasks_dir)
        pending = "[ ]" in text or "Status: **pending human review**" in text
        missing_identity = "Reviewer: \n" in text or "Reviewed at (UTC): \n" in text
        stale = len(sample) != text.count("## [") or any(
            f"`{fixture['id']}` ({fixture['family']})" not in text
            or hashlib.sha256(path.read_bytes()).hexdigest() not in text
            for path, fixture in sample
        )
        if pending or missing_identity or stale:
            print(f"human audit incomplete: {args.output}")
            return 1
        print(f"human audit complete: {args.output}")
        return 0
    args.output.write_text(render(select(args.tasks_dir)), encoding="utf-8")
    print(f"wrote pending human audit: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
