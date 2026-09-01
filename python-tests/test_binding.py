"""Tests for the `tokenfold` Python binding (crates/tokenfold-py).

Covers the Python binding's acceptance criteria:
  - `compress_openai_payload(payload, policy=...)` works from Python >= 3.9
  - `CompressionPolicy(target_tokens=..., mode=CompressionMode.BALANCED)` constructor
  - `result.report.saved_tokens`, `.estimator`, `.status` accessible
  - `TokenFoldError` variants map to the correct Python exception classes
"""

import json
from pathlib import Path

import pytest

import tokenfold
from tokenfold import (
    CompressionMode,
    CompressionPolicy,
    CompressionResult,
    ConfigError,
    EstimatorError,
    InputFormat,
    InternalError,
    InvalidInputError,
    LossyPath,
    RetrievalError,
    SafetyError,
    Status,
    TokenFoldError,
    compress,
    compress_anthropic_payload,
    compress_messages,
    compress_openai_payload,
    inspect,
    retrieve,
)

# 40 heterogeneous events (different event types carry different fields) plus one planted
# 503/success:false/retries:7 incident at index 23 -- the same fixture examples/lossy_pruning.py
# uses. Heterogeneity is deliberate: a uniform array gets folded into columnar form by the
# lossless pipeline before json_prune ever sees it, which would exercise nothing here.
INCIDENT_FEED = (
    Path(__file__).parent.parent / "examples" / "incident_feed.json"
).read_bytes()

OPENAI_PAYLOAD = json.dumps(
    {
        "model": "gpt-4.1",
        "messages": [
            {"role": "system", "content": "You are a terse assistant. " * 20},
            {"role": "user", "content": "first question " * 20},
            {"role": "assistant", "content": "first answer " * 20},
            {"role": "user", "content": "What is 2+2?"},
        ],
    }
).encode("utf-8")

ANTHROPIC_PAYLOAD = json.dumps(
    {
        "model": "claude-sonnet-5",
        "system": "You are a terse assistant. " * 20,
        "messages": [
            {"role": "user", "content": "first question " * 20},
            {"role": "assistant", "content": "first answer " * 20},
            {"role": "user", "content": "What is 2+2?"},
        ],
    }
).encode("utf-8")


# ---------------------------------------------------------------------------
# compress_openai_payload / compress_anthropic_payload
# ---------------------------------------------------------------------------


def test_compress_openai_payload_works_from_python():
    result = compress_openai_payload(OPENAI_PAYLOAD, target_tokens=50)
    assert isinstance(result, CompressionResult)
    assert isinstance(result.payload, bytes)
    # The compressed payload must still be valid JSON with a messages array.
    parsed = json.loads(result.payload)
    assert "messages" in parsed


def test_compress_anthropic_payload_works_from_python():
    result = compress_anthropic_payload(ANTHROPIC_PAYLOAD, target_tokens=50)
    assert isinstance(result, CompressionResult)
    assert isinstance(result.payload, bytes)


def test_compress_openai_payload_accepts_a_policy():
    policy = CompressionPolicy(target_tokens=50, mode=CompressionMode.BALANCED)
    result = compress_openai_payload(OPENAI_PAYLOAD, policy=policy)
    assert isinstance(result, CompressionResult)


# ---------------------------------------------------------------------------
# CompressionPolicy constructor
# ---------------------------------------------------------------------------


def test_compression_policy_constructor():
    policy = CompressionPolicy(target_tokens=12_000, mode=CompressionMode.BALANCED)
    assert policy.target_tokens == 12_000
    assert policy.mode == CompressionMode.BALANCED


def test_compression_policy_accepts_mode_as_a_string():
    policy = CompressionPolicy(mode="aggressive")
    assert policy.mode == CompressionMode.AGGRESSIVE


def test_compression_policy_rejects_disabling_secret_redaction():
    with pytest.raises(ConfigError):
        CompressionPolicy(disable=["secret_redaction"])


# ---------------------------------------------------------------------------
# result.report.{saved_tokens,estimator,status} accessible
# ---------------------------------------------------------------------------


def test_report_saved_tokens_estimator_and_status_are_accessible():
    result = compress(OPENAI_PAYLOAD, format=InputFormat.OPENAI_JSON, target_tokens=50)
    report = result.report
    assert isinstance(report.saved_tokens, int)
    assert report.estimator is not None
    assert report.estimator.backend in ("heuristic", "tiktoken", "huggingface", "anthropic")
    assert isinstance(report.status, Status)
    assert report.status in (
        Status.COMPRESSED,
        Status.PASSTHROUGH,
        Status.BEST_EFFORT,
        Status.UNREACHABLE_TARGET,
    )


def test_report_exposes_the_rest_of_the_fields_too():
    result = compress(OPENAI_PAYLOAD, format=InputFormat.OPENAI_JSON, target_tokens=50)
    report = result.report
    assert report.original_tokens >= report.compressed_tokens
    assert report.saved_tokens == report.original_tokens - report.compressed_tokens
    assert 0.0 <= report.savings_pct <= 100.0
    assert report.mode == "balanced"
    assert isinstance(report.raw, dict)
    assert "transforms" in report.raw


def test_compression_result_convenience_methods():
    result = compress(OPENAI_PAYLOAD, format=InputFormat.OPENAI_JSON, target_tokens=50)
    assert result.saved_pct() == result.report.savings_pct
    assert isinstance(result.is_over_budget(), bool)


# ---------------------------------------------------------------------------
# inspect(): dry run, payload unchanged, report reflects the would-be result
# ---------------------------------------------------------------------------


def test_inspect_does_not_modify_the_payload():
    result = inspect(OPENAI_PAYLOAD, format=InputFormat.OPENAI_JSON, target_tokens=50)
    assert result.payload == OPENAI_PAYLOAD
    assert result.report.saved_tokens >= 0


# ---------------------------------------------------------------------------
# compress_messages
# ---------------------------------------------------------------------------


def test_compress_messages_returns_message_oriented_fields():
    messages = [
        {"role": "system", "content": "You are terse. " * 20},
        {"role": "user", "content": "first question " * 20},
        {"role": "assistant", "content": "first answer " * 20},
        {"role": "user", "content": "What is 2+2?"},
    ]
    result = compress_messages(
        messages, model="gpt-4.1", token_budget=50, mode=CompressionMode.BALANCED
    )
    assert isinstance(result.messages, list)
    assert result.tokens_before >= result.tokens_after
    assert result.tokens_saved == result.tokens_before - result.tokens_after
    assert isinstance(result.transforms_applied, list)
    assert result.retrieval_hashes == []


# ---------------------------------------------------------------------------
# Enum ALL_CAPS naming (public API contract: no PascalCase leakage)
# ---------------------------------------------------------------------------


def test_enum_variant_names_are_all_caps():
    assert CompressionMode.CONSERVATIVE is not None
    assert CompressionMode.BALANCED is not None
    assert CompressionMode.AGGRESSIVE is not None

    assert InputFormat.AUTO is not None
    assert InputFormat.OPENAI_JSON is not None
    assert InputFormat.ANTHROPIC_JSON is not None
    assert InputFormat.PLAIN_TEXT is not None
    assert InputFormat.COMMAND_OUTPUT is not None
    assert InputFormat.GIT_DIFF is not None

    assert Status.COMPRESSED is not None
    assert Status.PASSTHROUGH is not None
    assert Status.BEST_EFFORT is not None
    assert Status.UNREACHABLE_TARGET is not None

    assert LossyPath.HEURISTIC is not None

    # No PascalCase leakage.
    assert not hasattr(CompressionMode, "Balanced")
    assert not hasattr(Status, "Compressed")
    assert not hasattr(LossyPath, "Heuristic")


# ---------------------------------------------------------------------------
# Error hierarchy (every error variant subclasses TokenFoldError)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "exc_cls",
    [InvalidInputError, SafetyError, EstimatorError, ConfigError, InternalError, RetrievalError],
)
def test_every_subclass_derives_from_tokenfold_error(exc_cls):
    assert issubclass(exc_cls, TokenFoldError)
    assert issubclass(TokenFoldError, Exception)


def test_config_error_raised_for_an_unknown_mode_string():
    with pytest.raises(ConfigError):
        compress(OPENAI_PAYLOAD, mode="not_a_real_mode")


def test_config_error_raised_for_an_unknown_format_string():
    with pytest.raises(ConfigError):
        compress(OPENAI_PAYLOAD, format="not_a_real_format")


def test_invalid_input_error_raised_for_unsupported_message_value_types():
    class Unsupported:
        pass

    with pytest.raises(InvalidInputError):
        compress_messages([{"role": "user", "content": Unsupported()}])


def test_tokenfold_error_is_the_catchall_base():
    with pytest.raises(TokenFoldError):
        compress(OPENAI_PAYLOAD, mode="nonsense")


def test_module_exports_the_full_error_hierarchy():
    for name in (
        "TokenFoldError",
        "InvalidInputError",
        "SafetyError",
        "EstimatorError",
        "ConfigError",
        "InternalError",
        "RetrievalError",
    ):
        assert hasattr(tokenfold, name)


# ---------------------------------------------------------------------------
# inspect(): side-effect-free for real, not merely in what it returns
# ---------------------------------------------------------------------------


def test_inspect_never_writes_to_the_retrieval_store(tmp_path, monkeypatch):
    """Round-5 external review: `inspect()` substituted the original payload back into the
    result but ran core compression under the ordinary persistence policy, so inspecting with
    `store_originals=True` still wrote the full payload to disk. `dry_run` now sets
    `policy.preview`, which is what actually stops the write inside core."""
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    policy = CompressionPolicy(store_originals=True, retrieval_backend="filesystem")

    result = inspect(OPENAI_PAYLOAD, policy=policy, format=InputFormat.OPENAI_JSON)

    assert result.payload == OPENAI_PAYLOAD
    assert result.report.raw["retrieval"] is None, "a preview must not claim a persist happened"
    assert not any(tmp_path.rglob("*.bin")), "a preview must not persist anything to disk"

    # Control: the same policy on the real `compress()` path DOES persist -- otherwise the
    # assertions above would pass for the wrong reason (nothing being stored under any path).
    compress(OPENAI_PAYLOAD, policy=policy, format=InputFormat.OPENAI_JSON)
    assert any(tmp_path.rglob("*.bin")), "store_originals must still work on a real run"


# ---------------------------------------------------------------------------
# Lossy JSON pruning + retrieve()
# ---------------------------------------------------------------------------


def test_compression_policy_accepts_lossy_settings():
    policy = CompressionPolicy(
        lossy=LossyPath.HEURISTIC,
        lossy_ratio=0.4,
        lossy_preserve=["events"],
    )
    assert policy.lossy == LossyPath.HEURISTIC
    assert policy.lossy_ratio == 0.4
    assert policy.lossy_preserve == ["events"]


def test_compression_policy_accepts_lossy_as_a_string():
    policy = CompressionPolicy(lossy="heuristic")
    assert policy.lossy == LossyPath.HEURISTIC


def test_compression_policy_lossy_defaults_to_none():
    policy = CompressionPolicy()
    assert policy.lossy is None
    assert policy.lossy_ratio == 0.3
    assert policy.lossy_preserve == []


def test_lossy_requires_a_durable_retrieval_backend():
    """Mirrors budget.rs's `lossy_refuses_memory_retrieval_backend`: a lossy run with no
    durable receipt is real data loss, not "lossy but recoverable" -- CompressionPolicy's own
    validate() must refuse this from Python exactly like it does from Rust/the CLI."""
    with pytest.raises(ConfigError):
        CompressionPolicy(lossy=LossyPath.HEURISTIC, retrieval_backend="memory")


def test_lossy_prunes_the_incident_feed_and_the_planted_incident_survives(tmp_path):
    policy = CompressionPolicy(
        retrieval_store_path=str(tmp_path),
        lossy=LossyPath.HEURISTIC,
        lossy_ratio=0.35,
    )
    lossless = compress(INCIDENT_FEED, format=InputFormat.JSON)
    lossy = compress(INCIDENT_FEED, format=InputFormat.JSON, policy=policy)

    assert lossy.report.saved_tokens > lossless.report.saved_tokens, (
        "lossy pruning must save strictly more than the lossless pipeline alone on this "
        "fixture -- if this regresses, the fixture no longer exercises json_prune (see the "
        "marker-overhead-vs-item-size lesson in the project's own lossy feature notes)"
    )

    compressed = json.loads(lossy.payload)
    events = compressed["events"]
    dropped = [e for e in events if isinstance(e, dict) and "$tf_ref" in e]
    kept = [e for e in events if isinstance(e, dict) and "$tf_ref" not in e]
    assert dropped, "this fixture must exercise lossy pruning"

    incident = [e for e in kept if e.get("status_code") == 503]
    assert incident, (
        "the planted incident must survive pruning -- structural failure signal outranks "
        "position in the selection ranking"
    )


def test_retrieve_recovers_a_dropped_event(tmp_path):
    policy = CompressionPolicy(
        retrieval_store_path=str(tmp_path),
        lossy=LossyPath.HEURISTIC,
        lossy_ratio=0.35,
    )
    lossy = compress(INCIDENT_FEED, format=InputFormat.JSON, policy=policy)
    compressed = json.loads(lossy.payload)
    dropped = [e for e in compressed["events"] if isinstance(e, dict) and "$tf_ref" in e]
    assert dropped, "fixture must produce at least one dropped event to retrieve"

    marker = dropped[0]["$tf_ref"]
    restored = retrieve(marker["hash"], namespace=marker["namespace"], policy=policy)

    original_events = json.loads(INCIDENT_FEED)["events"]
    assert json.loads(restored) in original_events, (
        "retrieved bytes must be one of the original, untouched events"
    )


def test_retrieve_via_policy_matches_explicit_kwargs(tmp_path):
    """`retrieve()`'s `policy=` is sugar for the same namespace/backend/store_path used to
    compress -- both call shapes must resolve to the same store and the same bytes."""
    policy = CompressionPolicy(
        retrieval_store_path=str(tmp_path),
        lossy=LossyPath.HEURISTIC,
        lossy_ratio=0.35,
    )
    lossy = compress(INCIDENT_FEED, format=InputFormat.JSON, policy=policy)
    marker = next(
        e["$tf_ref"]
        for e in json.loads(lossy.payload)["events"]
        if isinstance(e, dict) and "$tf_ref" in e
    )

    via_policy = retrieve(marker["hash"], namespace=marker["namespace"], policy=policy)
    via_kwargs = retrieve(
        marker["hash"],
        namespace=marker["namespace"],
        backend="filesystem",
        retrieval_store_path=str(tmp_path),
    )
    assert via_policy == via_kwargs


def test_retrieve_accepts_a_serialized_json_marker(tmp_path):
    policy = CompressionPolicy(
        retrieval_store_path=str(tmp_path),
        lossy=LossyPath.HEURISTIC,
        lossy_ratio=0.35,
    )
    lossy = compress(INCIDENT_FEED, format=InputFormat.JSON, policy=policy)
    marker = next(
        event
        for event in json.loads(lossy.payload)["events"]
        if isinstance(event, dict) and "$tf_ref" in event
    )

    restored = retrieve(json.dumps(marker), policy=policy)
    assert json.loads(restored) in json.loads(INCIDENT_FEED)["events"]


def test_retrieve_raises_for_a_missing_hash(tmp_path):
    with pytest.raises(RetrievalError):
        retrieve("0" * 64, namespace="default", retrieval_store_path=str(tmp_path))


def test_retrieval_error_is_a_tokenfold_error():
    assert issubclass(RetrievalError, TokenFoldError)


def test_inspect_previews_lossy_without_writing_anything(tmp_path):
    """Same contract as `test_inspect_never_writes_to_the_retrieval_store`, for the lossy path
    specifically: a preview must never perform a real `RetrievalStore` write, even when it's
    the lossy stage (not `store_originals`) driving the persistence."""
    policy = CompressionPolicy(
        retrieval_store_path=str(tmp_path),
        lossy=LossyPath.HEURISTIC,
        lossy_ratio=0.35,
    )
    preview = inspect(INCIDENT_FEED, format=InputFormat.JSON, policy=policy)
    assert preview.payload == INCIDENT_FEED
    assert not any(tmp_path.rglob("*.bin")), "a lossy preview must not persist anything to disk"

    compress(INCIDENT_FEED, format=InputFormat.JSON, policy=policy)
    assert any(tmp_path.rglob("*.bin")), "the real run must still persist"
