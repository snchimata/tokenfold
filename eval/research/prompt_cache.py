#!/usr/bin/env python3
"""Measure byte-prefix cache eligibility and simple repeated-input economics."""

import argparse
import json
from pathlib import Path


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("original", type=Path)
    parser.add_argument("compressed", type=Path)
    parser.add_argument("--prefix-bytes", type=int, required=True)
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--input-usd-per-million", type=float, required=True)
    parser.add_argument("--cached-input-multiplier", type=float, required=True)
    args = parser.parse_args(argv)
    if args.prefix_bytes < 0 or args.repeats < 1 or not 0 <= args.cached_input_multiplier <= 1:
        parser.error("prefix-bytes/repeats must be non-negative and multiplier must be in [0,1]")
    original, compressed = args.original.read_bytes(), args.compressed.read_bytes()
    prefix = original[: args.prefix_bytes]
    identical = compressed.startswith(prefix) and len(prefix) == args.prefix_bytes
    tokens = lambda data: (len(data) + 3) // 4
    raw_tokens, compressed_tokens, prefix_tokens = map(
        tokens, (original, compressed, prefix)
    )
    unit = args.input_usd_per_million / 1_000_000
    raw_cost = raw_tokens * args.repeats * unit
    compressed_cost = compressed_tokens * args.repeats * unit
    cached_cost = compressed_cost
    if identical and args.repeats > 1:
        cached_cost -= (
            prefix_tokens
            * (args.repeats - 1)
            * unit
            * (1 - args.cached_input_multiplier)
        )
    print(
        json.dumps(
            {
                "prefix_byte_identical": identical,
                "prefix_bytes": args.prefix_bytes,
                "estimator": "ceil(bytes/4)",
                "raw_repeated_cost_usd": raw_cost,
                "compressed_repeated_cost_usd": compressed_cost,
                "compressed_with_cache_cost_usd": cached_cost,
                "savings_vs_raw_usd": raw_cost - cached_cost,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
