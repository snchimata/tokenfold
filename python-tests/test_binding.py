"""Contract tests for the Tokenfold Python binding."""

import json
from pathlib import Path

import pytest

import tokenfold


def test_public_surface_is_the_v2_contract():
    assert tokenfold.Preset.BALANCED is not None
    assert tokenfold.Encoding.TOON is not None
    assert tokenfold.InputFormat.JSON is not None
    assert not hasattr(tokenfold, "CompressionMode")
    assert not hasattr(tokenfold, "LossyPath")
    assert not hasattr(tokenfold, "compress_messages")


def test_compress_returns_bytes_and_v2_receipt():
    result = tokenfold.compress(
        '{ "items": [1, 2, 3] }',
        format=tokenfold.InputFormat.JSON,
        preset=tokenfold.Preset.AGGRESSIVE,
    )
    assert isinstance(result.payload, bytes)
    assert result.text == result.payload.decode("utf-8")
    assert result.report.schema_version == "2.0"
    assert result.report.preset == "aggressive"
    assert result.report.raw["output_encoding"] == "json"


def test_inspect_returns_receipt_not_payload():
    receipt = tokenfold.inspect(b"\x00\xff\x01", format=tokenfold.InputFormat.PLAIN_TEXT)
    assert isinstance(receipt, tokenfold.CompressionReport)
    assert receipt.schema_version == "2.0"
    assert not hasattr(receipt, "payload")


def test_toon_round_trip():
    value = {"users": [{"id": 1, "active": True}, {"id": 2, "active": False}]}
    result = tokenfold.compress(
        json.dumps(value), format=tokenfold.InputFormat.JSON, encoding=tokenfold.Encoding.TOON
    )
    assert result.report.raw["encoding"]["roundtrip_verified"] is True
    assert json.loads(tokenfold.decode(result.payload, from_format="toon")) == value


def test_pruning_policy_validation():
    policy = tokenfold.PruningPolicy(keep_ratio=0.5, preserve_paths=["events"])
    assert policy.keep_ratio == 0.5
    assert policy.preserve_paths == ["events"]
    with pytest.raises(tokenfold.ConfigError):
        tokenfold.PruningPolicy(keep_ratio=0)


def test_hard_target_error_contains_receipt():
    with pytest.raises(tokenfold.BudgetUnmetError) as raised:
        tokenfold.compress(
            '{"protected":"content that cannot reach zero tokens"}',
            format="json",
            target_tokens=1,
            require_target=True,
        )
    assert raised.value.receipt.schema_version == "2.0"
    assert raised.value.receipt.raw["budget"]["status"] in {"best_effort", "unreachable"}


def test_pruned_evidence_can_be_retrieved(tmp_path):
    source = Path(__file__).parents[1] / "examples" / "incident_feed.json"
    result = tokenfold.compress(
        source.read_bytes(),
        format="json",
        target_tokens=50,
        pruning=tokenfold.PruningPolicy(
            keep_ratio=0.05,
            retrieval_store=tmp_path,
            retrieval_namespace="py-test",
        ),
    )
    marker = next(
        node["$tf_ref"]
        for node in json.loads(result.payload)["events"]
        if isinstance(node, dict) and "$tf_ref" in node
    )
    assert tokenfold.retrieve(marker, retrieval_store=tmp_path, namespace="py-test")


def test_invalid_inputs_are_typed_errors():
    with pytest.raises(tokenfold.InvalidInputError):
        tokenfold.compress("not json", format="json")
    with pytest.raises(tokenfold.ConfigError):
        tokenfold.compress("text", preset="unknown")
    with pytest.raises(tokenfold.RetrievalError):
        tokenfold.retrieve("0" * 64, retrieval_store=Path("missing-store"))
