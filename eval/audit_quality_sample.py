#!/usr/bin/env python3
"""Create/check a deterministic one-fixture-per-family human quality audit."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
from datetime import datetime, timezone
from pathlib import Path


def _commit_exists(commit: str) -> bool:
    """True when `commit` resolves in this repository.

    This keeps the provenance check honest: without it, any well-formed
    40-hex string passes, so a typo or fabricated hash looks reviewed.
    Runs `git cat-file -e`, which only verifies the object exists locally --
    it does not attest the commit is on any branch or that the reviewer
    actually inspected it.
    """
    try:
        subprocess.run(
            ["git", "cat-file", "-e", f"{commit}^{{commit}}"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return True
    except (OSError, subprocess.SubprocessError):
        return False


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
        "Reviewed commit: ",
        "Generator command: ",
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


def _valid_audit_metadata(text: str) -> bool:
    """Validate explicit provenance, not the truth of a human attestation."""
    fields = {}
    for label in ("Reviewer", "Reviewed at (UTC)", "Reviewed commit", "Generator command"):
        matches = re.findall(r"^" + re.escape(label) + r":[ \t]*([^\r\n]*)$", text, re.MULTILINE)
        if len(matches) != 1 or not matches[0].strip():
            return False
        fields[label] = matches[0].strip()
    if fields["Reviewer"].casefold() in {"developer", "reviewer", "unknown", "pending", "todo"}:
        return False
    if len(fields["Reviewer"]) < 2:
        return False
    if not re.fullmatch(r"[0-9a-f]{40}", fields["Reviewed commit"]) or set(fields["Reviewed commit"]) == {"0"}:
        return False
    stamp = fields["Reviewed at (UTC)"]
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|\+00:00)", stamp):
        return False
    try:
        reviewed = datetime.fromisoformat(stamp.replace("Z", "+00:00"))
        if reviewed > datetime.now(timezone.utc):
            return False
    except ValueError:
        return False
    return _commit_exists(fields["Reviewed commit"])


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tasks-dir", type=Path, default=Path(__file__).parent / "tasks/v04")
    parser.add_argument("--output", type=Path, default=Path(__file__).parent / "tasks/v04/HUMAN_AUDIT.md")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--force", action="store_true", help="explicitly replace an existing audit template")
    args = parser.parse_args(argv)
    if args.check:
        text = args.output.read_text(encoding="utf-8")
        sample = select(args.tasks_dir)
        pending = "[ ]" in text or "Status: **complete**" not in text
        missing_identity = not _valid_audit_metadata(text)
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
    if args.output.exists() and not args.force:
        print(f"refusing to overwrite existing audit without --force: {args.output}")
        return 1
    args.output.write_text(render(select(args.tasks_dir)), encoding="utf-8")
    print(f"wrote pending human audit: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
