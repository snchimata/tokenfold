"""Keep core-only quickstart checks offline and the explicit v3 path pinned."""
import ast
import builtins
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import re
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
NOTEBOOK = ROOT / "examples/quickstart.ipynb"


def cells():
    return json.loads(NOTEBOOK.read_bytes())["cells"]


def select_cell():
    return next("".join(c["source"]) for c in cells()
                if c["cell_type"] == "code" and "RUN_SELECT_V3 =" in "".join(c["source"]))


def test_notebook_sources_compile_and_have_no_saved_outputs():
    for i, cell in enumerate(cells()):
        if cell["cell_type"] == "code":
            ast.parse("".join(cell["source"]), filename=f"notebook-cell-{i}")
            assert cell["outputs"] == [] and cell["execution_count"] is None
    source = select_cell()
    assert "RUN_SELECT_V3 = True" in source
    assert re.search(r'SELECT_V3_MODEL_REVISION = "[0-9a-f]{64}"', source)
    assert re.search(r'SELECT_V3_MANIFEST_SHA256 = "[0-9a-f]{64}"', source)
    assert "truncation=True" not in source and "peft" not in source


def test_core_only_notebook_executes_without_model_imports_downloads_or_dispatch(monkeypatch):
    monkeypatch.chdir(ROOT)
    original = builtins.__import__
    forbidden = {"torch", "transformers", "peft", "huggingface_hub",
                 "tokenfold_select", "tokenfold_select_v3", "onnxruntime", "tokenizers"}

    def offline_import(name, *args, **kwargs):
        assert name.split(".")[0] not in forbidden, f"Unexpected model import: {name}"
        return original(name, *args, **kwargs)

    namespace = {"__name__": "__main__"}
    with patch("builtins.__import__", offline_import), \
         patch("subprocess.run", side_effect=AssertionError("core-only check must not dispatch a model")), \
         redirect_stdout(io.StringIO()):
        for i, cell in enumerate(cells()):
            if cell["cell_type"] == "code":
                source = "".join(cell["source"]).replace("RUN_SELECT_V3 = True", "RUN_SELECT_V3 = False", 1)
                exec(compile(source, f"notebook-cell-{i}", "exec"), namespace)
    assert namespace["RUN_SELECT_V3"] is False


def test_v3_relevance_failure_keeps_every_original_group():
    source = next("".join(c["source"]) for c in cells() if c["cell_type"] == "code"
                  and "".join(c["source"]).startswith("def validate_select_fixture("))
    namespace = {}
    exec(compile(source, "notebook-validator", "exec"), namespace)
    request = {"model_revision": "a" * 64, "groups": [
        {"id": "payments", "text": "Port 8443."}, {"id": "health", "text": "Ready."},
    ]}
    response = {"schema_version": 1, "model_revision": request["model_revision"],
                "scores": [["payments", 1.0], ["health", 2.0]]}
    result = namespace["validate_select_fixture"](request, response, expected_top_group="payments")
    assert result["response_contract_passed"] and not result["fixture_relevance_passed"]
    assert result["retained_groups"] == request["groups"] and not result["qualification"]
