"""Fail before publication if a supported registry package has version drift."""
import ast
import json
from pathlib import Path
import tomllib

root = Path(__file__).resolve().parents[1]


def toml(path):
    return tomllib.loads((root / path).read_text(encoding="utf8"))


version = toml("Cargo.toml")["workspace"]["package"]["version"]
for path in (root / "crates").glob("*/Cargo.toml"):
    manifest = toml(path)
    assert manifest["package"]["version"] == {"workspace": True}, path
    for dep in manifest.get("dependencies", {}).values():
        if isinstance(dep, dict) and "path" in dep and "version" in dep:
            assert dep["version"] == version, path
for package in toml("Cargo.lock")["package"]:
    if package["name"].startswith("tokenfold-") and "source" not in package:
        assert package["version"] == version, package["name"]
npm = json.loads((root / "packages/tokenfold/package.json").read_bytes())
lock = json.loads((root / "packages/tokenfold/package-lock.json").read_bytes())
assert npm["version"] == lock["version"] == lock["packages"][""]["version"] == version
assert len(npm["optionalDependencies"]) == 5
assert set(npm["optionalDependencies"].values()) == {version}
assert lock["packages"][""]["optionalDependencies"] == npm["optionalDependencies"]
for name in npm["optionalDependencies"]:
    entry = lock["packages"]["node_modules/" + name]
    assert entry["version"] == version, name
assert toml("packages/tokenfold-select/pyproject.toml")["project"]["version"] == version
module = ast.parse((root / "packages/tokenfold-select/src/tokenfold_select/__init__.py").read_text(encoding="utf8"))
assert next(ast.literal_eval(node.value) for node in module.body if isinstance(node, ast.Assign)
            and any(isinstance(t, ast.Name) and t.id == "__version__" for t in node.targets)) == version
assert toml("crates/tokenfold-py/pyproject.toml")["project"]["dynamic"] == ["version"]
print(f"All registry package versions agree: {version}")
