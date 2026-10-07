"""Research-only client for a caller-owned resident Qwen OpenAI loopback server.

The declared digest is NOT remote weight attestation. Freeze the owned server,
runtime and weights separately. No downloads, daemon launch, credentials or retries.
"""
import http.client
import json
import math
import subprocess
import sys
import time
import hashlib
from pathlib import Path

MAX_BYTES = 1024 * 1024
MODELS = {"Qwen/Qwen3.5-0.8B", "Qwen/Qwen3.5-2B", "Qwen/Qwen3.5-4B"}


class ModelError(ValueError):
    pass  # Only fixed reason codes; never a server body or exception text.


def unique_object(pairs):
    value = dict(pairs)
    if len(value) != len(pairs):
        raise ModelError("invalid_response")
    return value


def direct_request(port, path, body, timeout):
    if type(port) is not int or not 1 <= port <= 65535 or path not in {"models", "chat/completions", "apply-template", "tokenize"}:
        raise ModelError("route_refused")
    data = None if body is None else json.dumps(body).encode()
    if data is not None and len(data) > MAX_BYTES:
        raise ModelError("request_byte_limit")
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    try:
        route = "/" + path if path in {"apply-template", "tokenize"} else "/v1/" + path
        connection.request("GET" if data is None else "POST", route,
                           body=data, headers={"Content-Type": "application/json"})
        response = connection.getresponse()
        if response.status != 200:
            raise ModelError("http_error")  # No redirects or response-body export.
        data = response.read(MAX_BYTES + 1)
        if len(data) > MAX_BYTES:
            raise ModelError("response_byte_limit")
        value = json.loads(data, object_pairs_hook=unique_object)
        if not isinstance(value, dict):
            raise ModelError("invalid_response")
        return value
    except ModelError:
        raise
    except (ValueError, OSError, http.client.HTTPException):
        raise ModelError("transport_failed") from None
    finally:
        connection.close()


def template_accounting(port, body, timeout=10, *, transport=None):
    """Explicit resident-tokenizer diagnostic; never bypasses chat preflight."""
    transport = transport or bounded_request
    if not isinstance(body, dict) or not isinstance(body.get("messages"), list):
        raise ModelError("invalid_messages")
    start = time.perf_counter()
    value = transport(port, "apply-template", body, timeout)
    template_ms = (time.perf_counter() - start) * 1000
    prompt = value.get("prompt") if isinstance(value, dict) else None
    if not isinstance(prompt, str) or not prompt or len(prompt.encode()) > MAX_BYTES:
        raise ModelError("invalid_template")
    start = time.perf_counter()
    value = transport(port, "tokenize", {"content": prompt, "add_special": True, "parse_special": True}, timeout)
    tokens = value.get("tokens") if isinstance(value, dict) else None
    if not isinstance(tokens, list) or not tokens or any(type(token) is not int or token < 0 for token in tokens):
        raise ModelError("invalid_tokenizer_response")
    return {"prompt_tokens": len(tokens), "apply_template_wall_ms": template_ms,
            "tokenize_wall_ms": (time.perf_counter() - start) * 1000,
            "limitation": "Server-declared template/tokenizer result, not weights/runtime attestation, "
                          "summary inference or quality; client chat preflight is unchanged."}


def bounded_request(port, path, body, timeout):
    # Killable client bounds trickling responses; disconnection cannot cancel GPU kernels.
    try:
        result = subprocess.run([sys.executable, str(Path(__file__).resolve())],
                                input=json.dumps({"port": port, "path": path, "body": body,
                                                  "timeout": timeout}),
                                encoding="utf-8", capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise ModelError("client_deadline") from None
    if result.returncode:
        raise ModelError("client_failed")
    envelope = json.loads(result.stdout, object_pairs_hook=unique_object)
    if not envelope.get("ok"):
        raise ModelError("request_failed")
    return envelope["value"]


class NativeModel:
    def __init__(self, name, digest, port, max_calls, timeout, output_tokens, context_tokens,
                 transport=bounded_request, *, native_template_preflight=False):
        if (not isinstance(name, str) or name not in MODELS or not isinstance(digest, str) or len(digest) != 64
                or any(c not in "0123456789abcdef" for c in digest)
                or type(port) is not int or not 1 <= port <= 65535
                or type(max_calls) is not int or max_calls < 1
                or type(output_tokens) is not int or not 1 <= output_tokens <= 8192
                or type(context_tokens) is not int or not output_tokens < context_tokens <= 16384
                or type(timeout) not in (int, float) or not math.isfinite(timeout) or not 0 < timeout <= 30
                or type(native_template_preflight) is not bool):
            raise ModelError("invalid_profile")
        self.name, self.digest, self.port = name, digest, port
        self.max_calls, self.timeout = max_calls, timeout
        self.output_tokens, self.context_tokens = output_tokens, context_tokens
        self.transport, self.calls, self.stop_reason = transport, [], None
        self.native_template_preflight, self.accounting_calls = native_template_preflight, []
        self.verify()

    def verify(self):
        value = self.transport(self.port, "models", None, self.timeout)
        entries = value.get("data")
        if (not isinstance(entries, list) or sum(isinstance(m, dict) and m.get("id") == self.name
                                                for m in entries) != 1):
            raise ModelError("model_alias_mismatch")
        return self.digest  # Caller declaration only, not observed weight identity.

    def chat(self, messages, seed, *, response_schema=None):
        if self.stop_reason is not None:
            raise ModelError(self.stop_reason)
        if len(self.calls) >= self.max_calls:
            raise ModelError("call_budget")
        if (type(seed) is not int or not isinstance(messages, list) or not messages
                or any(not isinstance(m, dict) or set(m) != {"role", "content"}
                       or not isinstance(m["role"], str) or m["role"] not in {"system", "user", "assistant"}
                       or not isinstance(m["content"], str) or not m["content"] for m in messages)):
            raise ModelError("invalid_messages")
        body = {"model": self.name, "messages": messages, "stream": False,
                "max_tokens": self.output_tokens, "temperature": 0, "seed": seed,
                "chat_template_kwargs": {"enable_thinking": False}}
        if response_schema is not None:
            if not isinstance(response_schema, dict):
                raise ModelError("invalid_schema")
            body["response_format"] = {"type": "json_schema", "json_schema": {
                "name": "tokenfold_research", "strict": True, "schema": response_schema}}
        # Conservative bytes-as-token preflight; never silently truncate source.
        if not self.native_template_preflight and len(json.dumps(body, ensure_ascii=False).encode()) + self.output_tokens + 1024 > self.context_tokens:
            raise ModelError("context_allowance")
        try:
            self.verify()
        except (ValueError, OSError, subprocess.SubprocessError):
            self.stop_reason = "model_validation_failed"
            raise ModelError("model_validation_failed") from None
        accounting = None
        inference_timeout = self.timeout
        if self.native_template_preflight:
            # Seal caller-owned lists/schema before accounting; preserve roles and insertion order.
            try:
                wire = json.dumps(body, ensure_ascii=False, allow_nan=False)
                body = json.loads(wire)
            except (ValueError, TypeError):
                raise ModelError("invalid_body") from None
            accounting = {"status": "failed", "reason": None, "cost": None, "wall_ms": None,
                          "body_sha256": hashlib.sha256(wire.encode()).hexdigest(), "requests": []}
            self.accounting_calls.append(accounting)
            began = time.perf_counter()
            deadline = began + self.timeout
            def traced(port, path, payload, timeout):
                remaining = deadline - time.perf_counter()
                if remaining <= 0:
                    raise ModelError("client_deadline")
                event = {"path": path, "status": "failed", "cost": None, "wall_ms": None}
                accounting["requests"].append(event)
                start = time.perf_counter()
                try:
                    value = self.transport(port, path, payload, min(timeout, remaining))
                    event["status"] = "ok"
                    return value
                finally:
                    event["wall_ms"] = (time.perf_counter() - start) * 1000
            try:
                accounting.update(template_accounting(self.port, body, self.timeout, transport=traced))
                accounting["limitation"] = "Opt-in server-declared accounting; no weights/runtime or semantic proof. " \
                    "Accounting and generation share the remaining request deadline; client disconnect is not server cancellation."
                if json.dumps(body, ensure_ascii=False, allow_nan=False) != wire:
                    raise ModelError("body_changed")
                if accounting["prompt_tokens"] + self.output_tokens + 1024 > self.context_tokens:
                    accounting["reason"] = "context_allowance"
                    raise ModelError("context_allowance")
                inference_timeout = deadline - time.perf_counter()
                if inference_timeout <= 0:
                    raise ModelError("client_deadline")
                accounting["status"] = "ok"
            except (ValueError, OSError, subprocess.SubprocessError):
                if accounting["reason"] == "context_allowance":
                    raise ModelError("context_allowance") from None
                accounting["reason"] = self.stop_reason = "template_preflight_failed"
                raise ModelError(self.stop_reason) from None
            finally:
                accounting["wall_ms"] = (time.perf_counter() - began) * 1000
        row = {"status": "failed", "usage": None, "cost": None, "reason": None,
               "attempted": True, "wall_ms": None}
        if accounting is not None:
            row["template_accounting"] = accounting
        self.calls.append(row)
        start = time.perf_counter()
        try:
            result = self.transport(self.port, "chat/completions", body, inference_timeout)
            usage = result.get("usage")
            if isinstance(usage, dict) and all(type(usage.get(k)) is int and usage[k] >= 0
                                             for k in ("prompt_tokens", "completion_tokens")):
                row["usage"] = {"input_tokens": usage["prompt_tokens"], "output_tokens": usage["completion_tokens"]}
                if usage["completion_tokens"] > self.output_tokens:
                    row["reason"] = "output_allowance_exceeded"
                    raise ModelError(row["reason"])
                if usage["prompt_tokens"] + usage["completion_tokens"] > self.context_tokens:
                    row["reason"] = "context_allowance_exceeded"
                    raise ModelError(row["reason"])
            if accounting is not None and (row["usage"] is None or
                    row["usage"]["input_tokens"] != accounting["prompt_tokens"]):
                row["reason"] = "prompt_count_mismatch"
                raise ModelError(row["reason"])
            if result.get("model") != self.name:
                raise ModelError("model_alias_mismatch")
            choices = result.get("choices")
            if not isinstance(choices, list) or len(choices) != 1 or not isinstance(choices[0], dict):
                raise ModelError("invalid_response")
            choice = choices[0]
            if choice.get("finish_reason") != "stop":
                raise ModelError("incomplete_response")
            message = choice.get("message")
            content = message.get("content") if isinstance(message, dict) else None
            if not isinstance(content, str) or not content.strip() or len(content.encode()) > MAX_BYTES:
                raise ModelError("invalid_response")
            row["status"] = "ok"
            return content.strip()
        except (ValueError, OSError, subprocess.SubprocessError) as exc:
            safe_reasons = {"model_alias_mismatch", "incomplete_response", "invalid_response",
                            "client_deadline", "client_failed", "request_failed", "transport_failed",
                            "response_byte_limit", "request_byte_limit"}
            reason = str(exc) if isinstance(exc, ModelError) and str(exc) in safe_reasons else "generation_failed"
            row["reason"] = row["reason"] or reason
            self.stop_reason = row["reason"]  # No retry/queued work after uncertain inference.
            raise ModelError(row["reason"]) from None
        finally:
            row["wall_ms"] = round((time.perf_counter() - start) * 1000, 3)


if __name__ == "__main__":
    try:
        request = json.loads(sys.stdin.read(MAX_BYTES + 1), object_pairs_hook=unique_object)
        value = direct_request(**request)
        print(json.dumps({"ok": True, "value": value}))
    except Exception:
        print('{"ok":false}')  # Never emit traceback, source, server body or credentials.
