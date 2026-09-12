"""Regression checks for audit provenance and non-destructive generation (stdlib only)."""
import subprocess
import tempfile
from pathlib import Path

import audit_quality_sample as audit


def _head_commit() -> str:
    """A commit that resolves in this checkout (works on shallow CI clones)."""
    return subprocess.run(
        ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    ).stdout.strip()


HEAD = _head_commit()
VALID = "\n".join([
    "Reviewer: Test Reviewer (@fixture-only)",
    "Reviewed at (UTC): 2026-01-01T00:00:00Z",
    "Reviewed commit: " + HEAD,
    "Generator command: python eval/audit_quality_sample.py --output pending.md",
])


def test_metadata():
    assert audit._valid_audit_metadata(VALID)
    for label in ("Reviewer", "Reviewed at (UTC)", "Reviewed commit", "Generator command"):
        lines = VALID.splitlines()
        i = next(i for i, line in enumerate(lines) if line.startswith(label + ":"))
        lines[i] = label + ": \t"
        assert not audit._valid_audit_metadata("\n".join(lines)), label
        assert not audit._valid_audit_metadata(VALID + "\n" + VALID.splitlines()[i]), label
    for stamp in ("09/01/2026 13:00", "2026-01-01", "2026-01-01T00:00:00", "2026-13-01T00:00:00Z"):
        assert not audit._valid_audit_metadata(VALID.replace("2026-01-01T00:00:00Z", stamp))
    assert not audit._valid_audit_metadata(VALID.replace("Test Reviewer (@fixture-only)", "Developer"))
    # Well-formed but nonexistent / all-zero commits must fail: git cat-file cannot resolve them.
    assert not audit._valid_audit_metadata(VALID.replace(HEAD, "a" * 40))
    assert not audit._valid_audit_metadata(VALID.replace(HEAD, "0" * 40))


def test_existing_audit_is_not_overwritten():
    with tempfile.TemporaryDirectory() as root:
        path = Path(root) / "audit.md"
        path.write_text("existing human work", encoding="utf-8")
        assert audit.main(["--output", str(path), "--tasks-dir", root]) == 1
        assert path.read_text(encoding="utf-8") == "existing human work"


def test_fixture_digest_ignores_platform_line_endings():
    with tempfile.TemporaryDirectory() as root:
        path = Path(root) / "fixture.json"
        path.write_bytes(b'{\r\n  "id": "fixture"\r\n}\r\n')
        crlf = audit._fixture_digest(path)
        path.write_bytes(b'{\n  "id": "fixture"\n}\n')
        assert audit._fixture_digest(path) == crlf


if __name__ == "__main__":
    test_metadata()
    test_existing_audit_is_not_overwritten()
    test_fixture_digest_ignores_platform_line_endings()
    print("ok: audit metadata fails closed and generation preserves existing work")
