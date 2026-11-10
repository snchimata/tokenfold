"""Checks that must work without the checkout, model packages or network."""
import importlib.metadata
import json
from pathlib import Path
import subprocess
import sys


def test_installed_offline_base_operation(tmp_path):
    code = r'''
import builtins, json, sys
original = builtins.__import__
forbidden = {"torch", "transformers", "tokenizers", "huggingface_hub", "tokenfold_select_runtime", "tokenfold_select", "tokenfold_select_v3"}
def guarded(name, *args, **kwargs):
    assert name.split(".")[0] not in forbidden, name
    return original(name, *args, **kwargs)
builtins.__import__ = guarded
def offline(event, args):
    assert event not in {"socket.connect", "socket.getaddrinfo", "socket.bind", "subprocess.Popen", "os.system"}, event
sys.addaudithook(offline)
import tokenfold
from tokenfold import lowlevel
assert not hasattr(tokenfold, "compress")
assert tokenfold.fit("offline text", target_tokens=20).require_met() == "offline text"
assert lowlevel.compress("{\"x\":1}", format="json").report.schema_version == "2.0"
print(tokenfold.__file__)
'''
    completed = subprocess.run([sys.executable, "-I", "-c", code], cwd=tmp_path, text=True, capture_output=True, check=True)
    assert "site-packages" in completed.stdout
    assert "crates/tokenfold-py/python" not in completed.stdout.replace("\\", "/")


def test_installed_metadata_typing_and_abi():
    distribution = importlib.metadata.distribution("tokenfold")
    assert distribution.metadata["Requires-Python"] == ">=3.12"
    names = {str(path).replace("\\", "/") for path in distribution.files}
    assert "tokenfold/py.typed" in names and "tokenfold/_native.pyi" in names
    assert "tokenfold/types.py" in names
    assert any(path.endswith("/LICENSE") for path in names)
    assert not any("torch" in requirement.lower() for requirement in distribution.requires or ())
    wheel = distribution.read_text("WHEEL")
    assert "cp312-abi3" in wheel


def test_installed_precise_typing_fixtures(tmp_path):
    import shutil
    fixtures = Path(__file__).parent / "typing"
    for name in ("sdk_valid.py", "sdk_invalid.py"):
        shutil.copyfile(fixtures / name, tmp_path / name)
    checked = subprocess.run([sys.executable, "-m", "pyright", "--pythonpath", sys.executable, "--outputjson", "sdk_valid.py", "sdk_invalid.py"], cwd=tmp_path, text=True, capture_output=True, check=False)
    diagnostics = json.loads(checked.stdout)
    errors = [item for item in diagnostics["generalDiagnostics"] if item["severity"] == "error"]
    assert checked.returncode == 1
    assert all(Path(item["file"]).name == "sdk_invalid.py" for item in errors)
    assert {item["range"]["start"]["line"] for item in errors} == {1, 2, 3, 4, 5}
    assert diagnostics["summary"]["warningCount"] == 0


def test_lightweight_select_extra_is_exact_and_no_ml_dependencies():
    distribution = importlib.metadata.distribution("tokenfold")
    requires = distribution.requires or []
    assert len(requires) == 1
    requirement, marker = requires[0].split(";")
    assert requirement.strip() == "tokenfold-select-runtime==0.6.0"
    assert marker.replace("'", '"').strip() == 'extra == "select"'


def test_installed_sdk_fit_example(tmp_path):
    example = Path(__file__).resolve().parents[1] / "examples/sdk_fit.py"
    completed = subprocess.run([sys.executable, "-I", str(example)], cwd=tmp_path,
                               text=True, capture_output=True, check=True)
    assert completed.stdout.splitlines()[0] == '{"answer":42,"source":"example"}'
