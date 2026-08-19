"""End-to-end walkthrough of tokenfold's opt-in lossy array pruning.

    python examples/lossy_pruning.py
    TOKENFOLD_BIN=target/release/tokenfold python examples/lossy_pruning.py

PowerShell:

    $env:TOKENFOLD_BIN = "target/release/tokenfold.exe"
    python examples/lossy_pruning.py

This drives the `tokenfold` binary via subprocess so it runs against any install,
standard library only, no extra dependencies. The same knobs are available
in-process from the Python binding (`compress(..., lossy=LossyPath.HEURISTIC)`
plus `retrieve()`) and from the TypeScript package (`compress(input, { lossy:
"heuristic" })` plus `retrieve()`).

Everything runs against a throwaway retrieval store in a temp directory, so it
never touches the one your real runs use, and cleans up after itself.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).parent
FEED = HERE / "incident_feed.json"
UNIFORM = HERE / "api_response.json"

TOKENFOLD = os.environ.get("TOKENFOLD_BIN") or shutil.which("tokenfold")
if not TOKENFOLD:
    sys.exit(
        "tokenfold not found. Install it (`cargo install tokenfold-cli`), or point\n"
        "TOKENFOLD_BIN at a local build:\n"
        "    cargo build --release -p tokenfold-cli\n"
        "    TOKENFOLD_BIN=target/release/tokenfold python examples/lossy_pruning.py"
    )


def run(config, *args, capture_report=True):
    """Invoke the CLI. With `--json`, the payload goes to stdout and the receipt to
    stderr when `--output` is used, so the two never have to be untangled."""
    proc = subprocess.run(
        [TOKENFOLD, "--config", str(config), *args],
        capture_output=True,
        check=True,
    )
    if not capture_report:
        return proc.stdout
    stream = proc.stderr if any(a == "--output" for a in args) else proc.stdout
    return json.loads(stream)


def pct(report):
    return (
        f"{report['original_tokens']} -> {report['compressed_tokens']} tokens "
        f"({report['savings_pct']:.1f}%)"
    )


def main(work: Path) -> None:
    config = work / "config.toml"
    config.write_text(
        f'[retrieval]\nbackend = "filesystem"\nstore_path = "{(work / "store").as_posix()}"\n',
        encoding="utf-8",
    )
    common = ["compress", str(FEED), "--format", "json"]

    # 1. The ceiling structural compression reaches on this feed. Its 40 events are
    #    deliberately heterogeneous -- different event types carry different fields --
    #    so the columnar transform, which needs a uniform key set per row, cannot fold
    #    them and only whitespace minification applies.
    print("== 1. lossless only ==")
    r = run(config, "--json", *common, "--output", str(work / "lossless.json"))
    print(f"   {pct(r)}")
    applied = [t["id"] for t in r["transforms"] if t["status"] == "applied"]
    print(f"   applied: {', '.join(applied)}")

    lossy_flags = ["--lossy", "heuristic", "--lossy-ratio", "0.35"]

    # 2. --dry-run projects selection and token savings using the same content-
    #    safety checks, without opening the real filesystem store. A later real
    #    run may keep more rows if the filesystem itself refuses a write.
    print("\n== 2. preview what --lossy would do (writes nothing) ==")
    r = run(config, "--json", *common, "--dry-run", *lossy_flags)
    print(f"   projected: {pct(r)}")
    print(f"   retrieval: {r['retrieval']}  <- a preview never writes")
    assert not (work / "store").exists(), "preview must not create the store"

    print("\n== 3. compress for real ==")
    r = run(config, "--json", *common, "--output", str(work / "lossy.json"), *lossy_flags)
    prune = next(t for t in r["transforms"] if t["id"] == "json_prune")
    print(f"   {pct(r)}")
    print(f"   json_prune: {prune['status']}")
    print(
        f"   stored:     {r['retrieval']['marker_count']} entries, "
        f"ttl {r['retrieval']['ttl_seconds']}s"
    )
    # `quality` is present once a lossy transform really ran. Its metrics are absent
    # and gate_passed is false: no fidelity gate is baked in for json_prune yet, and
    # reporting an unmeasured 0.0 would be a stronger claim than "not measured".
    print(
        f"   quality:    gate_passed={r['quality']['gate_passed']}, "
        f"retention={r['quality']['quality_retention']}"
    )

    out = json.loads((work / "lossy.json").read_text(encoding="utf-8"))
    events = out["events"]
    dropped = [e for e in events if isinstance(e, dict) and "$tf_ref" in e]
    kept = [e for e in events if isinstance(e, dict) and "$tf_ref" not in e]
    assert dropped, "this fixture must exercise lossy pruning"
    print(f"   events:     {len(kept)} kept, {len(dropped)} replaced by markers")
    print("   note:       stored entries include one full-input backup")

    # 4. Ranking is driven by typed field checks -- a boolean status field that is
    #    false, a nonzero error/retry count, an HTTP status in 400-599 -- not by
    #    position or length, so the incident survives despite sitting mid-array.
    print("\n== 4. the event that matters survived ==")
    incident = [e for e in kept if e.get("status_code") == 503]
    assert incident, "the 503 incident must survive pruning"
    print(f"   {json.dumps(incident[0])[:160]}...")

    print("\n== 5. retrieve a dropped event ==")
    h = dropped[0]["$tf_ref"]["hash"]
    print(f"   retrieve {h}")
    restored = run(config, "retrieve", h, capture_report=False)
    restored_event = json.loads(restored)
    original_events = json.loads(FEED.read_text(encoding="utf-8"))["events"]
    assert restored_event in original_events, "retrieved bytes must contain an original event"
    print(f"   {restored.decode('utf-8').strip()[:160]}...")

    print("\n== 6. --lossy-preserve protects an array outright ==")
    payload = run(
        config, *common, "--quiet", *lossy_flags, "--lossy-preserve", "events",
        capture_report=False,
    )
    marker_count = payload.count(b"$tf_ref")
    assert marker_count == 0, "--lossy-preserve events must prevent every drop"
    print(f"   markers with --lossy-preserve events: {marker_count}")

    # 7. Uniform records fold well, so pruning them would be strictly worse.
    #    tokenfold computes both branches and keeps the better one.
    print("\n== 7. it declines when lossless already wins ==")
    uniform = ["compress", str(UNIFORM), "--format", "json", "--quiet"]
    a = run(config, *uniform, capture_report=False)
    b = run(config, *uniform, *lossy_flags, capture_report=False)
    assert a == b, "lossy must fall back to the lossless output here"
    print("   identical output -- nothing dropped, because dropping would not have helped")

    # 8. A ceiling showcase, not the default recommendation. Long rows amortize
    #    each retrieval marker's fixed cost, so an aggressive 2% keep hint can
    #    cross 90%. The planted 503 must still win the deterministic ranking.
    print("\n== 8. maximum-compression showcase (>90%) ==")
    words = (
        "architecture service request response cache database worker queue shard region "
        "latency throughput retry successful normal analysis record document content "
        "result relevance source context metadata"
    ).split()
    results = []
    for i in range(100):
        snippet = " ".join(
            words[(i * 7 + j * 11 + j * j) % len(words)] for j in range(800)
        )
        row = {
            "rank": i + 1,
            "title": f"Result {i + 1}: operational analysis for service shard {i % 17}",
            "url": f"https://example.test/docs/{i + 1}",
            "snippet": snippet,
            "success": True,
            "status_code": 200,
        }
        if i == 37:
            row.update(success=False, status_code=503, error_count=4)
        results.append(row)

    showcase = work / "max-showcase.json"
    showcase.write_text(
        json.dumps(
            {"query": "service reliability analysis", "results": results},
            separators=(",", ":"),
        ),
        encoding="utf-8",
    )
    showcase_out = work / "max-showcase.compact.json"
    r = run(
        config,
        "--json",
        "compress",
        str(showcase),
        "--format",
        "json",
        "--output",
        str(showcase_out),
        "--lossy",
        "heuristic",
        "--lossy-ratio",
        "0.02",
    )
    compact = json.loads(showcase_out.read_text(encoding="utf-8"))["results"]
    kept = [row for row in compact if "$tf_ref" not in row]
    markers = [row for row in compact if "$tf_ref" in row]
    assert r["savings_pct"] >= 90.0, "showcase must continue to substantiate the headline"
    assert any(row.get("status_code") == 503 for row in kept), "the planted 503 must survive"
    print(f"   {pct(r)}")
    print(f"   results:    {len(kept)} kept, {len(markers)} retrieval markers")
    print("   quality:    unvalidated -- this demonstrates the ceiling, not a safe default")


if __name__ == "__main__":
    tmp = Path(tempfile.mkdtemp(prefix="tokenfold_lossy_demo_"))
    try:
        main(tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
