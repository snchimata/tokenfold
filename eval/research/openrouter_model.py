"""Explicit, zero-price-only OpenRouter transport for synthetic research tasks."""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import subprocess
import sys
import time
from decimal import Decimal, InvalidOperation
from pathlib import Path

MODELS = {"stealth/space-bunny-alpha": "stealth",
          "nvidia/nemotron-3-ultra-550b-a55b:free": "nvidia",
          "nvidia/nemotron-3.5-lightning:free": "nvidia/nvfp4",
          "qwen/qwen3.8-27b:free": "modelrun/fp4",
          "nvidia/nemotron-3-super-120b-a12b:free": "nvidia",
          "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free": "nvidia",
          "dots-studio/dots-3-note-preview:free": "atlas-cloud/fp8",
          "poolside/laguna-s-2.1:free": "poolside/fp4",
          "poolside/laguna-xs-2.1:free": "poolside/fp8",
          "cohere/north-mini-code:free": "cohere",
          "liquid/lfm-2.5-2.6b:free": "liquid/fp8"}
MAX_BYTES = 1024 * 1024
STOP_REASONS = {"http_400", "http_401", "http_402", "http_403", "http_429", "response_route_mismatch"}


class ModelError(ValueError):
    """Safe reason code: never a provider response or credential."""


def read_key(path: Path) -> str:
    values = {}
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        match = re.match(r"^\s*(?:export\s+)?(OPENROUTER_API_KEY|openrouter_api_key)\s*=\s*(.*?)\s*$", line)
        if match:
            value = match[2]
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
                value = value[1:-1]
            values[match[1]] = value
    key = values.get("OPENROUTER_API_KEY") or values.get("openrouter_api_key")
    if not key or not re.fullmatch(r"[A-Za-z0-9_-]+", key):
        raise ModelError("credential_unavailable")
    return key


def ensure_windows_tls():
    if os.name == "nt" and "SystemRoot" not in os.environ:
        # Select clears inherited environment. Winsock and TLS need SystemRoot
        # BEFORE importing socket; derive only the native OS path, not credentials.
        import ctypes
        buffer = ctypes.create_unicode_buffer(32768)
        size = ctypes.windll.kernel32.GetWindowsDirectoryW(buffer, len(buffer))
        if not 0 < size < len(buffer):
            raise ModelError("windows_tls_unavailable")
        os.environ["SystemRoot"] = buffer.value


ensure_windows_tls()
import urllib.error
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def direct_request(path: str, body: dict | None, env_file: str, timeout: float) -> dict:
    allowed = {"chat/completions", *("models/" + name + "/endpoints" for name in MODELS)}
    if path not in allowed:
        raise ModelError("route_refused")
    ensure_windows_tls()
    data = None if body is None else json.dumps(body).encode("utf-8")
    if data is not None and len(data) > MAX_BYTES:
        raise ModelError("request_byte_limit")
    headers = {"Content-Type": "application/json"}
    if path == "chat/completions":
        headers["Authorization"] = "Bearer " + read_key(Path(env_file))
    request = urllib.request.Request("https://openrouter.ai/api/v1/" + path, data=data, headers=headers)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        with opener.open(request, timeout=timeout) as response:
            payload = response.read(MAX_BYTES + 1)
    except urllib.error.HTTPError as exc:
        raise ModelError("http_" + str(exc.code)) from None
    except OSError:
        raise ModelError("transport_failed") from None
    if len(payload) > MAX_BYTES:
        raise ModelError("response_byte_limit")
    try:
        result = json.loads(payload)
    except (ValueError, UnicodeError):
        raise ModelError("malformed_response") from None
    if not isinstance(result, dict) or result.get("error"):
        raise ModelError("provider_error")
    return result


def bounded_request(path: str, body: dict | None, env_file: str, timeout: float) -> dict:
    try:
        result = subprocess.run(
            [sys.executable, "-I", str(Path(__file__).resolve()), "--request"],
            input=json.dumps({"path": path, "body": body, "env_file": env_file, "timeout": timeout}),
            encoding="utf-8", capture_output=True, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        raise ModelError("client_deadline") from None
    if result.returncode:
        # Only allow our own reason codes out of the worker; never provider text.
        code = result.stdout.strip()
        if not re.fullmatch(r"(?:http_[0-9]{3}|[a-z_]{1,40})", code):
            code = "request_failed"
        raise ModelError(code)
    return json.loads(result.stdout)


def descriptor(name: str, response: dict) -> dict:
    data = response.get("data", {})
    if data.get("id") != name:
        raise ModelError("model_mismatch")
    endpoints = [e for e in data.get("endpoints", []) if e.get("tag") == MODELS[name]]
    if len(endpoints) != 1:
        raise ModelError("endpoint_unavailable")
    endpoint = endpoints[0]
    prices = endpoint.get("pricing", {})
    if not {"prompt", "completion"} <= prices.keys():
        raise ModelError("pricing_unavailable")
    try:
        if any(not Decimal(str(value)).is_finite() or Decimal(str(value)) != 0 for value in prices.values()):
            raise ModelError("paid_route_refused")
    except InvalidOperation:
        raise ModelError("pricing_unavailable") from None
    fields = ("name", "model_id", "context_length", "pricing", "provider_name", "tag",
              "quantization", "max_completion_tokens", "max_prompt_tokens", "supported_parameters")
    return {field: endpoint.get(field) for field in fields}


class FreeModel:
    def __init__(self, name: str, env_file: Path, max_calls: int, timeout: float,
                 output_tokens: int, context_tokens: int, transport=bounded_request,
                 expected_revision: str | None = None):
        if name not in MODELS:
            raise ModelError("model_not_approved")
        if (max_calls < 1 or output_tokens < 1 or context_tokens <= output_tokens
                or not math.isfinite(timeout) or timeout <= 0):
            raise ModelError("invalid_limits")
        self.name, self.env_file = name, str(env_file.resolve())
        self.max_calls, self.timeout = max_calls, timeout
        self.output_tokens, self.context_tokens = output_tokens, context_tokens
        self.transport, self.calls = transport, []
        self.stop_reason = None
        self.endpoint = self.verify()
        self.digest = hashlib.sha256(json.dumps(self.endpoint, sort_keys=True).encode()).hexdigest()
        if expected_revision is not None and expected_revision != self.digest:
            raise ModelError("endpoint_revision_changed")
        if (context_tokens > self.endpoint["context_length"]
                or output_tokens > self.endpoint["max_completion_tokens"]):
            raise ModelError("unsupported_limits")

    def verify(self) -> dict:
        return descriptor(self.name, self.transport("models/" + self.name + "/endpoints",
                                                  None, self.env_file, self.timeout))

    def chat(self, messages: list[dict], seed: int, *, response_schema: dict | None = None) -> str:
        if self.stop_reason is not None:
            raise ModelError(self.stop_reason)
        if len(self.calls) >= self.max_calls:
            raise ModelError("call_budget_exhausted")
        schema_bytes = len(json.dumps(response_schema).encode()) if response_schema is not None else 0
        if sum(len(m["content"].encode()) for m in messages) + schema_bytes + self.output_tokens + 1024 > self.context_tokens:
            raise ModelError("context_allowance_exceeded")
        if self.verify() != self.endpoint:
            raise ModelError("endpoint_revision_changed")
        if response_schema is not None and "structured_outputs" not in self.endpoint["supported_parameters"]:
            raise ModelError("structured_output_unsupported")
        body = {"model": self.name, "messages": messages, "stream": False,
                "max_tokens": self.output_tokens, "temperature": 0,
                "reasoning": {"effort": "low", "exclude": True},
                "provider": {"only": [MODELS[self.name]], "allow_fallbacks": False,
                             "max_price": {"prompt": 0, "completion": 0, "request": 0, "image": 0}}}
        if "seed" in self.endpoint["supported_parameters"]:
            body["seed"] = seed
        if response_schema is not None:
            body["response_format"] = {"type": "json_schema", "json_schema": {
                "name": "tokenfold_observation", "strict": True, "schema": response_schema}}
            body["provider"]["require_parameters"] = True
        row = {"status": "failed", "usage": None, "cost": None, "wall_ms": None,
               "provider": None, "reason": None, "attempted": True}
        self.calls.append(row)
        start = time.perf_counter()
        try:
            result = self.transport("chat/completions", body, self.env_file, self.timeout)
            row["provider"] = result.get("provider")
            usage = result.get("usage", {})
            counts = [usage.get(k) for k in ("prompt_tokens", "completion_tokens")]
            if all(type(n) is int and n >= 0 for n in counts):
                row["usage"] = dict(zip(("input_tokens", "output_tokens"), counts))
            cost = usage.get("cost")
            if type(cost) in (int, float) and math.isfinite(cost) and cost >= 0:
                row["cost"] = cost
                if cost != 0:
                    # Route has violated the free-only contract: stop all future calls.
                    self.max_calls = len(self.calls)
                    self.stop_reason = "unexpected_charge"
                    raise ModelError("unexpected_charge")
            if result.get("model") != self.name or row["provider"] != self.endpoint["provider_name"]:
                raise ModelError("response_route_mismatch")
            choices = result.get("choices", [])
            if len(choices) != 1 or choices[0].get("finish_reason") != "stop":
                raise ModelError("incomplete_response")
            content = choices[0].get("message", {}).get("content")
            if not isinstance(content, str) or not content.strip():
                raise ModelError("empty_response")
            row["status"] = "ok"
            return content.strip()
        except ModelError as exc:
            row["reason"] = str(exc)
            if str(exc) in STOP_REASONS:
                self.max_calls = len(self.calls)
                self.stop_reason = str(exc)
            raise
        finally:
            row["wall_ms"] = round((time.perf_counter() - start) * 1000, 3)


if __name__ == "__main__":
    if sys.argv[1:] != ["--request"]:
        raise SystemExit("Use acon_benchmark.py; this file is its bounded transport worker.")
    try:
        request = json.loads(sys.stdin.read(MAX_BYTES + 1))
        print(json.dumps(direct_request(**request)))
    except ModelError as exc:
        print(str(exc))
        raise SystemExit(1)
    except Exception:
        print("request_failed")
        raise SystemExit(1)
