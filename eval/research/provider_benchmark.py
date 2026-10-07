#!/usr/bin/env python3
"""Compare Tokenfold and Headroom on the same versioned JSON corpus."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

import tiktoken
import headroom.compression.universal as headroom_module
from headroom.compression.universal import compress as headroom_compress
from benchmark_provenance import checkout_provenance

ENCODING = tiktoken.get_encoding("o200k_base")


def count_tokens(text: str) -> int:
    return len(ENCODING.encode(text))


def saved_pct(after: int, before: int) -> float:
    return round((before - after) * 100 / before, 2) if before else 0.0


def run(command: list[str], source: str) -> str:
    process = subprocess.run(
        command,
        input=source,
        text=True,
        encoding="utf-8",
        capture_output=True,
        check=False,
        timeout=30,
    )
    if process.returncode:
        detail = process.stderr.strip() or f"exit code {process.returncode}"
        raise RuntimeError(f"{' '.join(command)}: {detail}")
    return process.stdout


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--headroom-revision", required=True)
    parser.add_argument("--headroom-root", required=True, type=Path)
    parser.add_argument("--tokenfold-revision", required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)

    provenance = checkout_provenance(args.headroom_root, args.headroom_revision,
                                     Path(headroom_module.__file__))
    if args.output and args.output.exists():
        parser.error("output already exists (never overwrite evidence)")

    root = Path(__file__).resolve().parents[2]
    actual_revision = subprocess.check_output(
        ["git", "-C", str(root), "rev-parse", "HEAD"], text=True, timeout=30
    ).strip()
    if args.tokenfold_revision != actual_revision:
        parser.error("tokenfold revision must match the current checkout")
    tokenfold_dirty = bool(subprocess.check_output(
        ["git", "-C", str(root), "status", "--porcelain"], timeout=30
    ))
    executable = (
        root
        / "target"
        / "release"
        / ("tokenfold.exe" if os.name == "nt" else "tokenfold")
    )
    if not executable.exists():
        parser.error(
            "build the release CLI first: cargo build --release --locked -p tokenfold-cli"
        )
    binary_hash = hashlib.sha256(executable.read_bytes()).hexdigest()

    args.manifest = args.manifest.resolve()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    if manifest.get("version") != 1:
        parser.error("manifest version must be 1")

    rows = []
    for case in manifest["cases"]:
        path = (args.manifest.parent / case["path"]).resolve()
        source_bytes = path.read_bytes()
        source = source_bytes.decode("utf-8")
        value = json.loads(source)
        before = count_tokens(source)

        tokenfold = run(
            [str(executable), "compress", "-", "--format", "json", "--quiet"], source
        )
        decoded = run([str(executable), "decode", "-"], tokenfold)
        headroom = headroom_compress(source).compressed
        try:
            headroom_value_equal = json.loads(headroom) == value
        except json.JSONDecodeError:
            headroom_value_equal = False

        tokenfold_tokens = count_tokens(tokenfold)
        headroom_tokens = count_tokens(headroom)
        rows.append(
            {
                "id": case["id"],
                "shape": case["shape"],
                "input": path.relative_to(root).as_posix(),
                "original_tokens": before,
                "source_sha256": hashlib.sha256(source_bytes).hexdigest(),
                "tokenfold": {
                    "tokens": tokenfold_tokens,
                    "saved_pct": saved_pct(tokenfold_tokens, before),
                    "exact_recovery": json.loads(decoded) == value,
                },
                "headroom": {
                    "tokens": headroom_tokens,
                    "saved_pct": saved_pct(headroom_tokens, before),
                    "json_value_equal": headroom_value_equal,
                },
                "token_winner": (
                    "tokenfold"
                    if tokenfold_tokens < headroom_tokens
                    else "headroom"
                    if headroom_tokens < tokenfold_tokens
                    else "tie"
                ),
            }
        )

    original = sum(row["original_tokens"] for row in rows)
    tokenfold = sum(row["tokenfold"]["tokens"] for row in rows)
    headroom = sum(row["headroom"]["tokens"] for row in rows)
    report = {
        "headroom_provenance": provenance,
        "tokenfold_binary_sha256": binary_hash,
        "tokenfold_worktree_dirty": tokenfold_dirty,
        "tokenizer": {"backend": "tiktoken", "encoding": "o200k_base", "exact": True},
        "headroom_revision": args.headroom_revision,
        "source_commit": args.tokenfold_revision,
        "manifest": args.manifest.relative_to(root).as_posix(),
        "files": rows,
        "totals": {
            "original_tokens": original,
            "tokenfold": {
                "tokens": tokenfold,
                "saved_pct": saved_pct(tokenfold, original),
            },
            "headroom": {
                "tokens": headroom,
                "saved_pct": saved_pct(headroom, original),
            },
            "tokenfold_wins": sum(row["token_winner"] == "tokenfold" for row in rows),
            "headroom_wins": sum(row["token_winner"] == "headroom" for row in rows),
        },
        "limitations": [
            "This compares each project's default local generic-JSON API, not hosted proxy behavior.",
            "Token counts use one external tokenizer so both outputs are measured identically.",
            "Binary hash identifies the executable, not proof it was built from the declared source revision.",
            "Tokenfold exactness is checked after decode; Headroom is checked as emitted JSON.",
        ],
    }
    rendered = json.dumps(report, indent=2) + "\n"
    if checkout_provenance(args.headroom_root, args.headroom_revision,
                           Path(headroom_module.__file__)) != provenance:
        raise ValueError("comparator implementation changed during benchmark")
    if hashlib.sha256(executable.read_bytes()).hexdigest() != binary_hash:
        raise ValueError("Tokenfold binary changed during benchmark")
    if args.output:
        with args.output.open("x", encoding="utf-8") as output:
            output.write(rendered)
    else:
        print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
