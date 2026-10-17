"""Keep the quickstart executable offline and its optional model path pinned."""
import ast
import builtins
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import re
import sys
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
NOTEBOOK = ROOT / "examples/quickstart.ipynb"


def cells():
    return json.loads(NOTEBOOK.read_bytes())["cells"]


def select_cell():
    return next("".join(c["source"]) for c in cells()
                if c["cell_type"] == "code" and "RUN_TOKENFOLD_SELECT =" in "".join(c["source"]))


def test_notebook_sources_compile_and_have_no_saved_outputs():
    for i, cell in enumerate(cells()):
        if cell["cell_type"] == "code":
            ast.parse("".join(cell["source"]), filename=f"notebook-cell-{i}")
            assert cell["outputs"] == [] and cell["execution_count"] is None
    source = select_cell()
    assert "RUN_TOKENFOLD_SELECT = False" in source
    assert re.search(r'SELECT_MODEL_REVISION = "[0-9a-f]{40}"', source)
    assert "truncation=True" not in source and "peft" not in source


def test_default_notebook_executes_without_model_imports_or_downloads(monkeypatch):
    monkeypatch.chdir(ROOT)
    original = builtins.__import__
    forbidden = {"torch", "transformers", "peft", "huggingface_hub",
                 "tokenfold_select", "onnxruntime", "tokenizers"}

    def offline_import(name, *args, **kwargs):
        assert name.split(".")[0] not in forbidden, f"Unexpected model import: {name}"
        return original(name, *args, **kwargs)

    namespace = {"__name__": "__main__"}
    with patch("builtins.__import__", offline_import), redirect_stdout(io.StringIO()):
        for i, cell in enumerate(cells()):
            if cell["cell_type"] == "code":
                exec(compile("".join(cell["source"]), f"notebook-cell-{i}", "exec"), namespace)
    assert namespace["RUN_TOKENFOLD_SELECT"] is False


def test_optional_cell_uses_pinned_onnx_profile_and_original_groups(tmp_path):
    calls = {}

    def download(repo, **kwargs):
        calls.update(repo=repo, **kwargs)
        return str(tmp_path)

    class Scorer:
        revision = "4dbdb5071e7d6316d8eb70dc161b1f2f77f555012f6616d7fa34311188e24cec"

        def __init__(self, directory, *, development):
            assert directory == tmp_path and development is True

        def score(self, request):
            calls["request"] = request
            return {"scores": [[g["id"], 0.0] for g in request["groups"]]}

    source = select_cell().replace("RUN_TOKENFOLD_SELECT = False", "RUN_TOKENFOLD_SELECT = True", 1)
    namespace = dict(json=json, Path=Path, EXAMPLES=ROOT / "examples")
    with patch.dict(sys.modules, huggingface_hub=SimpleNamespace(snapshot_download=download),
                    tokenfold_select=SimpleNamespace(OnnxScorer=Scorer)), patch.dict("os.environ"), redirect_stdout(io.StringIO()):
        exec(compile(source, "optional-notebook-cell", "exec"), namespace)
    assert calls["repo"] == "snchimata/tokenfold-select-v2"
    assert re.fullmatch("[0-9a-f]{40}", calls["revision"])
    assert "model.onnx" in calls["allow_patterns"] and "model.onnx.data" in calls["allow_patterns"]
    assert "model.safetensors" not in calls["allow_patterns"]
    request = calls["request"]
    assert request["model_revision"] == Scorer.revision
    assert [g["id"] for g in request["groups"]] == ["0", "1", "2"]
    events = json.loads((ROOT / "examples/incident_feed.json").read_bytes())["events"]
    assert request["groups"][0]["text"] == next(e["detail"] for e in events if e["event"] == "incident")
