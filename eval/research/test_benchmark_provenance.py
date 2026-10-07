"""Offline checkout identity checks; no provider or model calls."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from benchmark_provenance import checkout_provenance


class ProvenanceTests(unittest.TestCase):
    def test_checkout_identity_and_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()

            def git(*args):
                return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.DEVNULL)

            git("init")
            module = root / "compress.py"
            module.write_text("# fixture\n", encoding="utf-8")
            git("add", "compress.py")
            # Identity belongs only to this disposable fixture, not the user's repository.
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "-c", "commit.gpgsign=false", "commit", "-m", "Fixture")
            revision = git("rev-parse", "HEAD").decode().strip()
            receipt = checkout_provenance(root, revision, module)
            self.assertEqual(receipt["imported_module"], "compress.py")
            self.assertEqual(len(receipt["tracked_sha256"]["compress.py"]), 64)
            nested = root / "nested"
            nested.mkdir()
            with self.assertRaises(ValueError):
                checkout_provenance(nested, revision, nested / "compress.py")
            for bad_revision in ("a" * 40, "HEAD", "A" * 40):
                with self.assertRaises(ValueError):
                    checkout_provenance(root, bad_revision, module)
            with self.assertRaises(ValueError):
                checkout_provenance(root, revision, root.parent / "other.py")
            module.write_text("# changed\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                checkout_provenance(root, revision, module)
            git("checkout", "--", "compress.py")
            for flag in ("assume-unchanged", "skip-worktree"):
                git("update-index", "--" + flag, "compress.py")
                module.write_text("# hidden mutation\n", encoding="utf-8")
                self.assertEqual(git("status", "--porcelain"), b"")
                with self.assertRaisesRegex(ValueError, "index flags"):
                    checkout_provenance(root, revision, module)
                git("update-index", "--no-" + flag, "compress.py")
                git("checkout", "--", "compress.py")
                self.assertEqual(checkout_provenance(root, revision, module), receipt)
            (root / "extra.py").write_text("# injected\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                checkout_provenance(root, revision, root / "extra.py")


if __name__ == "__main__":
    unittest.main()
