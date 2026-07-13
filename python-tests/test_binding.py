"""Tests for the `tokenfold` Python binding (crates/tokenfold-py).

Covers roadmap.md F-041's acceptance criteria:
  - `compress_openai_payload(payload, policy=...)` works from Python >= 3.9
  - `CompressionPolicy(target_tokens=..., mode=CompressionMode.BALANCED)` constructor
  - `result.report.saved_tokens`, `.estimator`, `.status` accessible
  - `TokenFoldError` variants map to the correct Python exception classes
"""

import json

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
    SafetyError,
    Status,
    TokenFoldError,
    compress,
    compress_anthropic_payload,
    compress_messages,
    compress_openai_payload,
    inspect,
)

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
        "model": "claude-3-5-sonnet",
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
# Enum ALL_CAPS naming (INTERFACES.md section 5.4)
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

    # No PascalCase leakage.
    assert not hasattr(CompressionMode, "Balanced")
    assert not hasattr(Status, "Compressed")


# ---------------------------------------------------------------------------
# Error hierarchy (INTERFACES.md section 5.5 / roadmap.md F-003)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "exc_cls",
    [InvalidInputError, SafetyError, EstimatorError, ConfigError, InternalError],
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
    ):
        assert hasattr(tokenfold, name)
