"""Verify comparator checkout identity instead of trusting report labels (stdlib only)."""

import hashlib
import subprocess
from pathlib import Path


def checkout_provenance(root: Path, revision: str, imported_file: Path) -> dict:
    root, imported_file = root.resolve(), imported_file.resolve()
    if len(revision) != 40 or any(c not in "0123456789abcdef" for c in revision):
        raise ValueError("comparator revision must be a full lowercase commit hash")
    try:
        relative = imported_file.relative_to(root).as_posix()
    except ValueError:
        raise ValueError("comparator was imported outside the approved checkout") from None

    def git(*args):
        return subprocess.check_output(["git", "-C", str(root), *args], timeout=30)

    if Path(git("rev-parse", "--show-toplevel").decode().strip()).resolve() != root:
        raise ValueError("comparator root must be the checkout root")
    if git("rev-parse", "HEAD").decode().strip() != revision:
        raise ValueError("comparator checkout does not match revision")
    # git status can hide modified files carrying either index flag.
    entries = git("ls-files", "-v", "-z").decode("utf-8").split("\0")
    if any(entry and (entry[0].islower() or entry[0] == "S") for entry in entries):
        raise ValueError("comparator checkout must not hide tracked files with index flags")
    if git("status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("comparator checkout must be clean")
    tracked = git("ls-files", "-z").decode("utf-8").split("\0")
    if relative not in tracked:
        raise ValueError("imported comparator module is not tracked")
    hashes = {name: hashlib.sha256((root / name).read_bytes()).hexdigest()
              for name in tracked if name and (root / name).is_file()}
    return {"revision": revision, "imported_module": relative, "tracked_sha256": hashes}
