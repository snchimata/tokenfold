"""Fail before publication if an active registry package has version drift."""
import ast
import json
from pathlib import Path
import tomllib


def check(root: Path) -> str:
    def toml(path):
        return tomllib.loads((root / path).read_text(encoding="utf8"))
    def js(path):
        return json.loads((root / path).read_bytes())
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
    npm = js("packages/tokenfold/package.json")
    lock = js("packages/tokenfold/package-lock.json")
    assert npm["version"] == lock["version"] == lock["packages"][""]["version"] == version
    optional = npm["optionalDependencies"]
    assert optional and set(optional.values()) == {version}
    assert lock["packages"][""]["optionalDependencies"] == optional
    for name in optional:
        entry = lock["packages"]["node_modules/" + name]
        assert entry.get("version") == version, name
    natives = [js(path) for path in (root / "crates/tokenfold-node/npm").glob("*/package.json")]
    assert {m["name"] for m in natives} == {n for n in optional if n.startswith("@tokenfold/native-")}
    assert all(m["version"] == version for m in natives)
    runtime = toml("packages/tokenfold-select-runtime/pyproject.toml")["project"]
    assert runtime["version"] == version
    source = root / "packages/tokenfold-select-runtime/src/tokenfold_select_runtime/__init__.py"
    module = ast.parse(source.read_text(encoding="utf8"))
    assert next(ast.literal_eval(n.value) for n in module.body if isinstance(n, ast.Assign)
                and any(isinstance(t, ast.Name) and t.id == "__version__" for t in n.targets)) == version
    python = toml("crates/tokenfold-py/pyproject.toml")["project"]
    assert python["dynamic"] == ["version"]
    assert python["optional-dependencies"]["select"] == [f"tokenfold-select-runtime=={version}"]
    return version


if __name__ == "__main__":
    print(f"All active registry package versions agree: {check(Path(__file__).resolve().parents[1])}")
