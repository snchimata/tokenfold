#!/usr/bin/env python3
"""Compare TOON and Tokenfold with compact JSON on caller-supplied JSON data."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import time
from pathlib import Path

TOON_VERSION = "4.1.1"

try:  # Optional, as in the other evaluation harnesses.
    import tiktoken

    _ENCODING = tiktoken.get_encoding("o200k_base")

    def count_tokens(text: str) -> int:
        return len(_ENCODING.encode(text))

    TOKENIZER = {"backend": "tiktoken", "model": "o200k_base", "is_exact": True}
except Exception:  # noqa: BLE001 - optional research dependency

    def count_tokens(text: str) -> int:
        return (len(text.encode("utf-8")) + 3) // 4 if text else 0

    TOKENIZER = {"backend": "heuristic", "model": None, "is_exact": False}


def percent_delta(value: int, baseline: int) -> float | None:
    return round((value - baseline) * 100 / baseline, 2) if baseline else None


def run(command: list[str], source: str) -> tuple[str, float]:
    started = time.perf_counter()
    process = subprocess.run(
        command,
        input=source,
        text=True,
        encoding="utf-8",
        capture_output=True,
        check=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1_000
    if process.returncode:
        detail = process.stderr.strip() or f"exit code {process.returncode}"
        raise RuntimeError(f"{' '.join(command)}: {detail}")
    return process.stdout, elapsed_ms


def measure(command: list[str], source: str, runs: int) -> tuple[str, float]:
    outputs, timings = zip(*(run(command, source) for _ in range(runs)))
    if len(set(outputs)) != 1:
        raise RuntimeError(f"non-deterministic output from {' '.join(command)}")
    return outputs[0], round(statistics.median(timings), 3)


def measure_python(function, runs: int):
    outputs, timings = [], []
    for _ in range(runs):
        started = time.perf_counter()
        outputs.append(function())
        timings.append((time.perf_counter() - started) * 1_000)
    if any(output != outputs[0] for output in outputs[1:]):
        raise RuntimeError("non-deterministic Python codec output")
    return outputs[0], round(statistics.median(timings), 6)


def format_result(text, compact_tokens, compact_bytes, encode_ms, decode_ms) -> dict:
    tokens = count_tokens(text)
    byte_count = len(text.encode("utf-8"))
    return {
        "tokens": tokens,
        "bytes": byte_count,
        "vs_compact_json_percent": percent_delta(tokens, compact_tokens),
        "vs_compact_json_bytes_percent": percent_delta(byte_count, compact_bytes),
        "encode_median_ms": encode_ms,
        "decode_median_ms": decode_ms,
    }


def load_cases(inputs: list[Path], manifest: Path | None) -> list[dict]:
    if not manifest:
        return [{"id": path.stem, "path": path, "shape": None} for path in inputs]
    document = json.loads(manifest.read_text(encoding="utf-8"))
    if document.get("version") != 1 or not isinstance(document.get("cases"), list):
        raise ValueError(
            "TOON corpus manifest must contain version=1 and a cases array"
        )
    cases = []
    for case in document["cases"]:
        if not all(key in case for key in ("id", "path", "shape")):
            raise ValueError("each TOON corpus case needs id, path, and shape")
        cases.append({**case, "path": (manifest.parent / case["path"]).resolve()})
    return cases


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("inputs", nargs="*", type=Path, help="JSON files to benchmark")
    parser.add_argument("--manifest", type=Path, help="versioned corpus manifest")
    parser.add_argument(
        "--runs", type=int, default=3, help="codec runs per file (default: 3)"
    )
    parser.add_argument(
        "--output", type=Path, help="write the JSON report instead of stdout"
    )
    parser.add_argument("--tokenfold-revision", required=True)
    parser.add_argument(
        "--require-exact",
        action="store_true",
        help="fail unless tiktoken/o200k_base is installed",
    )
    args = parser.parse_args(argv)
    if args.runs < 1:
        parser.error("runs must be at least 1")
    if bool(args.inputs) == bool(args.manifest):
        parser.error("provide JSON inputs or --manifest, but not both")
    if args.require_exact and not TOKENIZER["is_exact"]:
        parser.error("exact counting requires the eval 'exact' extra (tiktoken)")

    research_dir = Path(__file__).resolve().parent
    repository_root = research_dir.parents[1]
    local_toon = (
        research_dir
        / "node_modules"
        / ".bin"
        / ("toon.cmd" if os.name == "nt" else "toon")
    )
    if local_toon.exists():
        toon, toon_runner = [str(local_toon)], "local npm dependency"
    else:
        npx = shutil.which("npx")
        if not npx:
            parser.error("run npm ci in eval/research or install npx")
        toon = [npx, "--yes", f"@toon-format/cli@{TOON_VERSION}"]
        toon_runner = "npx fallback"

    executable = "tokenfold.exe" if os.name == "nt" else "tokenfold"
    local = next(
        (
            path
            for path in (
                Path("target/release") / executable,
                Path("target/debug") / executable,
            )
            if path.exists()
        ),
        None,
    )
    tokenfold = (
        [str(local.resolve())]
        if local
        else ["cargo", "run", "--quiet", "-p", "tokenfold-cli", "--"]
    )

    try:
        cases = load_cases(args.inputs, args.manifest)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))
    rows = []
    for case in cases:
        path = case["path"]
        source = path.read_text(encoding="utf-8")
        value = json.loads(source)
        compact, compact_encode_ms = measure_python(
            lambda value=value: json.dumps(
                value, ensure_ascii=False, separators=(",", ":")
            ),
            args.runs,
        )
        compact_tokens = count_tokens(compact)
        compact_bytes = len(compact.encode("utf-8"))
        compact_value, compact_decode_ms = measure_python(
            lambda compact=compact: json.loads(compact), args.runs
        )

        tokenfold_text, tokenfold_ms = measure(
            tokenfold + ["compress", "-", "--format", "json", "--quiet"],
            source,
            args.runs,
        )
        tokenfold_decoded, tokenfold_decode_ms = measure(
            tokenfold + ["decode", "-", "--from", "json"], tokenfold_text, args.runs
        )
        tokenfold_value = json.loads(tokenfold_decoded)
        toon_text, toon_ms = measure(toon + ["--encode", "-"], source, args.runs)
        decoded, toon_decode_ms = measure(
            toon + ["--decode", "-"], toon_text, args.runs
        )
        toon_value = json.loads(decoded)

        rows.append(
            {
                "id": case["id"],
                "input": path.relative_to(repository_root).as_posix(),
                "shape": case["shape"],
                "compact_json": {
                    "tokens": compact_tokens,
                    "bytes": compact_bytes,
                    "encode_median_ms": compact_encode_ms,
                    "decode_median_ms": compact_decode_ms,
                    "json_value_equal": compact_value == value,
                },
                "tokenfold": {
                    **format_result(
                        tokenfold_text,
                        compact_tokens,
                        compact_bytes,
                        tokenfold_ms,
                        tokenfold_decode_ms,
                    ),
                    "exact_recovery": tokenfold_value == value,
                },
                "toon": {
                    **format_result(
                        toon_text,
                        compact_tokens,
                        compact_bytes,
                        toon_ms,
                        toon_decode_ms,
                    ),
                    "round_trip_equal": toon_value == value,
                },
            }
        )

    compact_tokens = sum(row["compact_json"]["tokens"] for row in rows)
    compact_bytes = sum(row["compact_json"]["bytes"] for row in rows)
    totals = {
        "compact_json": {
            "tokens": compact_tokens,
            "bytes": compact_bytes,
            "encode_median_ms_sum": round(
                sum(row["compact_json"]["encode_median_ms"] for row in rows), 3
            ),
            "decode_median_ms_sum": round(
                sum(row["compact_json"]["decode_median_ms"] for row in rows), 3
            ),
        }
    }
    for name in ("tokenfold", "toon"):
        tokens = sum(row[name]["tokens"] for row in rows)
        byte_count = sum(row[name]["bytes"] for row in rows)
        totals[name] = {
            "tokens": tokens,
            "bytes": byte_count,
            "vs_compact_json_percent": percent_delta(tokens, compact_tokens),
            "vs_compact_json_bytes_percent": percent_delta(byte_count, compact_bytes),
            "encode_median_ms_sum": round(
                sum(row[name]["encode_median_ms"] for row in rows), 3
            ),
            "decode_median_ms_sum": round(
                sum(row[name]["decode_median_ms"] for row in rows), 3
            ),
        }
    totals["tokenfold"]["wins_vs_toon"] = sum(
        row["tokenfold"]["tokens"] < row["toon"]["tokens"] for row in rows
    )
    report = {
        "tokenizer": TOKENIZER,
        "toon_cli_version": TOON_VERSION,
        "toon_runner": toon_runner,
        "source_commit": args.tokenfold_revision,
        "runs_per_input": args.runs,
        "files": rows,
        "totals": totals,
        "limitations": [
            "CLI startup is included in TOON and Tokenfold latency.",
            "Python JSON timings are in-process and are not directly comparable to CLI timings.",
        ],
    }
    rendered = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    else:
        print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
