"""Real-world quickstart for the `tokenfold` Python package.

Install the binding, then run this file:

    pip install tokenfold          # or: maturin develop -m crates/tokenfold-py/Cargo.toml
    python examples/quickstart.py

It shows the two things you'll actually do in an app:

  1. compress_messages(...) — hand it your OpenAI-style chat list, get a compressed
     list back plus exact token accounting, ready to send to a model.
  2. compress(...) — compress a raw request body (bytes/str) for any supported format.
  3. compress(..., format="JSON") — compress generic JSON *data* (API responses,
     records, logs), not just message payloads. Losslessly folds repeated keys and values.

Every call returns a typed report: before/after tokens, which transforms ran, and any
safety warnings. tokenfold never silently drops content — you get receipts.
"""

import json
from pathlib import Path

import tokenfold

# The same payload the Rust example uses (examples/openai_payload.json).
PAYLOAD = json.loads((Path(__file__).parent / "openai_payload.json").read_text())


def compress_a_message_list() -> None:
    """The common case: you have a `messages` list bound for the Chat Completions API."""
    result = tokenfold.compress_messages(
        PAYLOAD["messages"],
        model="gpt-4o",
        mode="BALANCED",
    )

    print("== compress_messages ==")
    print(f"tokens:     {result.tokens_before} -> {result.tokens_after} "
          f"({result.tokens_saved} saved, {result.savings_pct:.1f}%)")
    print(f"transforms: {', '.join(result.transforms_applied) or '(none applied)'}")

    # `result.messages` is the compressed list — send it straight to your provider:
    #   client.chat.completions.create(model="gpt-4o", messages=result.messages)
    print(f"messages:   {len(result.messages)} message(s) ready to send\n")


def compress_a_raw_body() -> None:
    """Compress a full request body (here, the whole OpenAI JSON incl. the tool schema)."""
    result = tokenfold.compress(
        json.dumps(PAYLOAD),
        format="OPENAI_JSON",
        mode="BALANCED",
    )
    report = result.report

    print("== compress (raw OpenAI body) ==")
    print(f"status:     {report.status}")
    print(f"tokens:     {report.original_tokens} -> {report.compressed_tokens} "
          f"({report.saved_tokens} saved, {report.savings_pct:.1f}%)")
    print(f"estimator:  {report.estimator.backend} (exact: {report.estimator.is_exact})")
    if report.warnings:
        for w in report.warnings:
            print(f"  warning: {w}")
    # result.payload is the compressed bytes; report.raw has the full JSON report.
    print(f"payload:    {len(result.payload)} bytes\n")


def compress_generic_json_data() -> None:
    """Compress a JSON API response (not a message payload)."""
    data = (Path(__file__).parent / "api_response.json").read_text()
    result = tokenfold.compress(data, format="JSON", mode="BALANCED")
    report = result.report

    print("== compress (generic JSON data) ==")
    print(f"tokens:     {report.original_tokens} -> {report.compressed_tokens} "
          f"({report.saved_tokens} saved, {report.savings_pct:.1f}%)")
    applied = [t["id"] for t in report.raw["transforms"] if t["status"] == "applied"]
    print(f"transforms: {', '.join(applied)}")
    # result.payload is the compressed JSON (columnar + value dictionary), losslessly
    # reversible — every stage is round-trip gated before it's applied.
    print(f"payload:    {len(result.payload)} bytes\n")


if __name__ == "__main__":
    compress_a_message_list()
    compress_a_raw_body()
    compress_generic_json_data()
