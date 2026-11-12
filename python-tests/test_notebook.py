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


def test_notebook_sources_compile_and_select_requires_explicit_approval():
    notebook_cells = cells()
    intro = "".join(notebook_cells[0]["source"])
    assert "tokenfold==0.6.0" in intro
    assert "unreleased" not in intro.lower()
    assert "locally built wheel" not in intro.lower()
    for i, cell in enumerate(notebook_cells):
        if cell["cell_type"] == "code":
            ast.parse("".join(cell["source"]), filename=f"notebook-cell-{i}")
            assert cell["outputs"] == []
    source = select_cell()
    assert 'os.environ.get("TOKENFOLD_RUN_SELECT") == "1"' in source
    assert 'TOKENFOLD_SELECT_RUNTIME_CONFIG' in source
    assert 'allow_selection=True' in source
    assert 'SelectRuntime' in source and 'experimental=True' in source
    assert 'subprocess.run' not in source and 'sys.path' not in source


def test_core_only_notebook_executes_without_model_imports_downloads_or_dispatch(monkeypatch):
    monkeypatch.chdir(ROOT)
    monkeypatch.delenv("TOKENFOLD_RUN_SELECT", raising=False)
    original = builtins.__import__
    forbidden = {"torch", "transformers", "peft", "huggingface_hub",
                 "tokenfold_select", "tokenfold_select_v3", "onnxruntime", "tokenizers"}
    def offline_import(name, *args, **kwargs):
        assert name.split(".")[0] not in forbidden, name
        return original(name, *args, **kwargs)
    namespace = {"__name__": "__main__"}
    with patch("builtins.__import__", offline_import), \
         patch("subprocess.run", side_effect=AssertionError("unexpected child")), \
         redirect_stdout(io.StringIO()):
        for i, cell in enumerate(cells()):
            if cell["cell_type"] == "code":
                exec(compile("".join(cell["source"]), f"notebook-cell-{i}", "exec"), namespace)
    assert namespace["RUN_SELECT_V3"] is False
    assert namespace["fit_result"].require_met() == "Keep this text unchanged."
