"""Exercise the current CLI, including decoding before structural evidence scoring."""
import json
from pathlib import Path
from unittest.mock import patch

import run_baselines as baseline


def test_cli_contract():
    assert baseline._TOKENFOLD_BIN, "build tokenfold-cli and set TOKENFOLD_BIN"
    assert baseline._lossy_smoke_checks() == []
    for name in ("log_qa_010", "code_build_error_012"):
        fixture = json.loads((Path(__file__).parent / "tasks/v04" / (name + ".json")).read_text(encoding="utf-8"))
        encoded = baseline.compress_tokenfold(fixture["source"], 100)
        assert encoded.startswith("__tf_logfold1__\n"), "regression must exercise log folding"
        assert baseline.decode_compressor_output(encoded) == fixture["source"]
        result = baseline.run_one_compressor(fixture, "deterministic-tokenfold", 0.5)
        assert result["available"] and result["task_success"] == 1.0, result
    with patch.object(baseline, "decode_compressor_output", return_value=None):
        assert not baseline.run_one_compressor(fixture, "deterministic-tokenfold", 0.5)["available"]
    with patch.object(baseline.subprocess, "run", side_effect=OSError("unavailable")):
        assert baseline.decode_compressor_output("example") is None


if __name__ == "__main__":
    test_cli_contract()
    print("ok: current CLI pruning, retrieval, preserve, and structural decoding")
