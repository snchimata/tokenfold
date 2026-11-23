"""Version checks use active source manifests, never a local model/archive."""
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseVersions(unittest.TestCase):
    def test_active_only_and_drift(self):
        spec = importlib.util.spec_from_file_location("versions", ROOT / "scripts/check-release-versions.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", *ROOT.glob("crates/*/Cargo.toml"),
                     *ROOT.glob("crates/tokenfold-node/npm/*/package.json"),
                     ROOT / "packages/tokenfold/package.json", ROOT / "packages/tokenfold/package-lock.json",
                     ROOT / "crates/tokenfold-py/pyproject.toml",
                     ROOT / "packages/tokenfold-select-runtime/pyproject.toml",
                     ROOT / "packages/tokenfold-select-runtime/src/tokenfold_select_runtime/__init__.py"]
            for path in paths:
                dest = root / path.relative_to(ROOT)
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, dest)
            module.check(root)
            manifest = root / "crates/tokenfold-node/npm/win32-x64/package.json"
            value = json.loads(manifest.read_text())
            value["version"] = "0.0.0"
            manifest.write_text(json.dumps(value))
            with self.assertRaises(AssertionError):
                module.check(root)


if __name__ == "__main__":
    unittest.main()
