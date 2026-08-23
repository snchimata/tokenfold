"""Keeps README.md's fixture-backed compression claim honest.

The README publishes the exact token counts the bundled API-response fixture
compresses to. Fixture edits and transform changes both move that number, so this
test re-measures it with the installed binding and fails on drift. It skips when
the repository files are absent (e.g. a wheel-only test run), because then there
is no claim to check.
"""

import json
import re
from pathlib import Path

import pytest

import tokenfold

REPO_ROOT = Path(__file__).resolve().parent.parent
README = REPO_ROOT / "README.md"
FIXTURE = REPO_ROOT / "examples" / "api_response.json"

CLAIM = re.compile(
    r"The bundled (\d+)-record API response reports ([\d,]+) → ([\d,]+) tokens,\s+"
    r"a \*\*([\d.]+)%",
)


def test_readme_fixture_claim_matches_measurement():
    if not (README.is_file() and FIXTURE.is_file()):
        pytest.skip("repository files not present; nothing to check")
    match = CLAIM.search(README.read_text(encoding="utf-8"))
    if match is None:
        pytest.skip("README no longer publishes the fixture claim")

    source = FIXTURE.read_text(encoding="utf-8")
    report = tokenfold.compress(source, format=tokenfold.InputFormat.JSON).report

    claimed = {
        "records": int(match.group(1)),
        "original": int(match.group(2).replace(",", "")),
        "compressed": int(match.group(3).replace(",", "")),
        "pct": float(match.group(4)),
    }
    measured = {
        "records": len(json.loads(source)["results"]),
        "original": report.original_tokens,
        "compressed": report.compressed_tokens,
        "pct": round(report.savings_pct, 1),
    }
    assert claimed == measured, (
        f"README claim {claimed} no longer matches measurement {measured}; "
        "update README.md (and re-run examples/quickstart.ipynb) in this change"
    )


INCIDENT_FIXTURE = REPO_ROOT / "examples" / "incident_feed.json"

INCIDENT_ROW = re.compile(
    r"\| (?:Lossless|`--lossy-ratio (0\.\d+)`) \| ([\d,]+) \| \*{0,2}([\d.]+)%\*{0,2} "
    r"\| ([\d,]+) \|"
)


def test_readme_incident_table_matches_measurement(tmp_path):
    if not (README.is_file() and INCIDENT_FIXTURE.is_file()):
        pytest.skip("repository files not present; nothing to check")
    claimed = INCIDENT_ROW.findall(README.read_text(encoding="utf-8"))
    if not claimed:
        pytest.skip("README no longer publishes the incident-feed table")

    source = INCIDENT_FIXTURE.read_text(encoding="utf-8")
    measured = []
    for ratio, *_ in claimed:
        if not ratio:  # the Lossless row
            report = tokenfold.compress(source, format=tokenfold.InputFormat.JSON).report
            kept = len(json.loads(source)["events"])
        else:
            policy = tokenfold.CompressionPolicy(
                retrieval_backend="filesystem",
                retrieval_store_path=str(tmp_path / ratio),
                lossy=tokenfold.LossyPath.HEURISTIC,
                lossy_ratio=float(ratio),
            )
            result = tokenfold.compress(
                source, policy=policy, format=tokenfold.InputFormat.JSON
            )
            report = result.report
            kept = sum(
                1 for e in json.loads(result.payload)["events"] if "$tf_ref" not in e
            )
        measured.append(
            (ratio, f"{report.compressed_tokens:,}", f"{report.savings_pct:.1f}", f"{kept}")
        )
    assert claimed == measured, (
        f"README incident-feed table {claimed} no longer matches measurement {measured}; "
        "update README.md (and re-run examples/quickstart.ipynb) in this change"
    )
