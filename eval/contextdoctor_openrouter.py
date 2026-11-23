#!/usr/bin/env python3
"""Opt-in data-only OpenRouter adapter: exact :free model/provider, no paid fallback.

Only the worker/catalog entry points perform network I/O. Never executes model
output. See eval/README.md for approval, pricing and billing-evidence limitations.
"""
from __future__ import annotations

import argparse
from decimal import Decimal, InvalidOperation
import json
import os
from pathlib import Path
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from contextdoctor import (SCHEMA_VERSION, ContractError, require, fields, header, integer,
                           decimal, string, write_json)

BASE_URL = "https://openrouter.ai/api/v1/"
KNOWN_PRICES = {"prompt", "completion", "request", "image", "web_search", "internal_reasoning",
                "input_cache_read", "input_cache_write"}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ContractError("provider_redirect_refused")


def validate_catalog(c):
    fields(c, "schema_version kind model canonical_model provider pricing supported_parameters", "free catalog")
    header(c, "openrouter_catalog")
    require(isinstance(c["model"], str) and re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+:free", c["model"]), "only exact :free model IDs are admitted")
    require(not c["model"].startswith("openrouter/"), "router models are refused")
    require(c["canonical_model"] in (c["model"], c["model"][:-5]), "unrecognized canonical model")
    require(isinstance(c["provider"], str) and re.fullmatch(r"[a-z0-9_.-]+(?:/[a-z0-9_.-]+)*", c["provider"]), "invalid exact provider tag")
    prices = c["pricing"]
    require(isinstance(prices, dict) and {"prompt", "completion"} <= set(prices) <= KNOWN_PRICES,
            "missing/unknown pricing category")
    for value in prices.values():
        require(decimal(value, "free price") == 0, "nonzero price refused")
    require(isinstance(c["supported_parameters"], list) and
            all(isinstance(p, str) for p in c["supported_parameters"]) and
            {"max_tokens", "temperature"} <= set(c["supported_parameters"]), "provider lacks required limits")


def catalog_from_api(model, provider, data):
    require(isinstance(data, dict) and isinstance(data.get("data"), dict), "invalid endpoints response")
    data = data["data"]
    require(data.get("id") == model, "endpoint model mismatch")
    endpoints = data.get("endpoints")
    require(isinstance(endpoints, list), "missing endpoint pricing")
    matches = [e for e in endpoints if isinstance(e, dict) and e.get("tag") == provider]
    require(len(matches) == 1, "exact provider endpoint unavailable/ambiguous")
    e = matches[0]
    require(isinstance(e.get("pricing"), dict), "missing endpoint pricing")
    pricing = dict(e["pricing"])
    if "discount" in pricing:
        discount = pricing.pop("discount")
        require(not isinstance(discount, bool) and decimal(str(discount), "catalog discount") == 0,
                "nonzero/unknown endpoint discount refused")
    c = {"schema_version": SCHEMA_VERSION, "kind": "contextdoctor.openrouter_catalog", "model": model,
         "canonical_model": model[:-5] if model.endswith(":free") else model, "provider": provider,
         "pricing": pricing, "supported_parameters": sorted(e.get("supported_parameters") or [])}
    validate_catalog(c)
    return c


def build_request(catalog, context, query, max_tokens, temperature):
    validate_catalog(catalog)
    require(isinstance(context, str) and isinstance(query, str), "text-only input required")
    integer(max_tokens, "max_tokens", 1, 4096)
    require(decimal(temperature, "temperature") <= 2, "invalid temperature")
    return {"model": catalog["model"], "messages": [{"role": "user", "content": context}, {"role": "user", "content": query}],
            "stream": False, "max_tokens": max_tokens, "temperature": float(temperature),
            "provider": {"only": [catalog["provider"]], "allow_fallbacks": False, "require_parameters": True,
                         "max_price": {"prompt": 0, "completion": 0, "request": 0, "image": 0}},
            "plugins": []}


def _provider_usage(response, catalog, max_tokens):
    require(isinstance(response, dict), "invalid_response")
    require(response.get("model") in (catalog["model"], catalog["canonical_model"]), "returned_model_mismatch")
    u = response.get("usage")
    normalized = None
    if u is not None:
        require(isinstance(u, dict), "invalid_usage")
        for key in ("prompt_tokens", "completion_tokens", "total_tokens"):
            integer(u.get(key), key)
        require(u["total_tokens"] == u["prompt_tokens"] + u["completion_tokens"], "usage_total_mismatch")
        require(u["completion_tokens"] <= max_tokens, "output_token_limit_violated")
        details = u.get("prompt_tokens_details")
        if details is not None:
            require(isinstance(details, dict), "invalid_cache_usage")
            require(set(details) <= {"cached_tokens", "cache_write_tokens", "audio_tokens", "video_tokens"}, "unknown_input_usage_category")
            require(not details.get("audio_tokens") and not details.get("video_tokens"), "nontext_usage_refused")
            cached, written = details.get("cached_tokens"), details.get("cache_write_tokens")
            # Missing cache-write is unknown, not zero; providers may omit the breakdown.
            if cached is not None and written is not None:
                integer(cached, "cached tokens")
                integer(written, "cache write tokens")
                require(cached + written <= u["prompt_tokens"], "overlapping_cache_usage")
                normalized = {"model": catalog["model"], "source": "provider_observed", "normalization": "disjoint_v1",
                              "snapshots": [{"input": u["prompt_tokens"] - cached - written, "cache_read": cached,
                                             "cache_write": written, "output": u["completion_tokens"]}]}
    return normalized


def normalize_response(response, catalog, max_tokens):
    validate_catalog(catalog)
    require(isinstance(response, dict) and not response.get("error"), "provider_error")
    require(response.get("model") in (catalog["model"], catalog["canonical_model"]), "returned_model_mismatch")
    string(response.get("id"), "request ID")
    choices = response.get("choices")
    require(isinstance(choices, list) and len(choices) == 1 and isinstance(choices[0], dict), "unexpected_choices")
    answer = choices[0]
    require(not answer.get("error") and answer.get("finish_reason") in ("stop", "length", "content_filter"), "unexpected_finish_reason")
    require(answer["finish_reason"] == "stop", "incomplete_final_answer")
    message = answer.get("message")
    require(isinstance(message, dict) and message.get("role") == "assistant" and not message.get("tool_calls") and not message.get("function_call"), "tool_output_refused")
    text = message.get("content")
    require(isinstance(text, str), "missing_output")
    normalized = _provider_usage(response, catalog, max_tokens)
    u = response.get("usage")
    if u is not None:
        require(u.get("is_byok") in (None, False), "BYOK_refused")
        if u.get("cost") is not None:
            require(not isinstance(u["cost"], bool) and Decimal(str(u["cost"])).is_finite() and Decimal(str(u["cost"])) == 0, "nonzero_reported_charge")
        for value in (u.get("cost_details") or {}).values():
            require(value is None or (not isinstance(value, bool) and Decimal(str(value)).is_finite() and Decimal(str(value)) == 0), "nonzero_upstream_charge")
        require(not any((u.get("server_tool_use") or {}).values()), "server_tools_refused")
    coverage_reason = "request_price_unreported" if "request" not in catalog["pricing"] else None
    if coverage_reason is None and normalized is not None:
        counters = normalized["snapshots"][0]
        if any(counters[metric] and price not in catalog["pricing"] for metric, price in
               (("cache_read", "input_cache_read"), ("cache_write", "input_cache_write"))):
            coverage_reason = "cache_price_unreported"
    return {"output": text, "request_id": response["id"], "usage": normalized, "cost_coverage_reason": coverage_reason,
            "returned_model": response["model"], "finish_reason": answer["finish_reason"], "receipt": response}


def http_json(path, body, key, timeout, limit):
    require(path.startswith("models/") or path == "chat/completions", "endpoint_refused")
    require(".." not in path and "?" not in path and "#" not in path, "endpoint_refused")
    headers = {"Content-Type": "application/json", "User-Agent": "Tokenfold-ContextDoctor-experimental"}
    if key:
        headers["Authorization"] = "Bearer " + key
    payload = None if body is None else json.dumps(body, allow_nan=False).encode("utf-8")
    request = urllib.request.Request(BASE_URL + path, data=payload, headers=headers,
                                     method="GET" if body is None else "POST")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=timeout) as response:
        raw = response.read(limit + 1)
    require(len(raw) <= limit, "response_output_limit")
    try:
        return json.loads(raw, parse_float=Decimal, parse_constant=lambda x: (_ for _ in ()).throw(ContractError("nonfinite_provider_number")))
    except (json.JSONDecodeError, UnicodeError) as exc:
        raise ContractError("invalid_provider_json") from exc


def _json_safe(value):
    if isinstance(value, Decimal):
        return format(value, "f")
    if isinstance(value, dict):
        return {k: _json_safe(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_json_safe(v) for v in value]
    return value


def worker(value):
    fields(value, "action catalog context query max_tokens temperature timeout output_limit", "worker input")
    c = value["catalog"]
    validate_catalog(c)
    integer(value["max_tokens"], "max tokens", 1, 4096)
    integer(value["output_limit"], "output limit", 1, 8 * 1024 * 1024)
    timeout = float(decimal(value["timeout"], "timeout"))
    require(timeout > 0, "invalid timeout")
    if value["action"] == "preflight":
        path = "models/" + urllib.parse.quote(c["model"], safe="/") + "/endpoints"
        fresh = catalog_from_api(c["model"], c["provider"], http_json(path, None, os.environ.get("OPENROUTER_API_KEY"), timeout, value["output_limit"]))
        require(fresh == c, "frozen_catalog_changed")
        return {"catalog": fresh}
    require(value["action"] == "request", "unknown worker action")
    key = os.environ.get("OPENROUTER_API_KEY")
    require(bool(key), "OPENROUTER_API_KEY_missing")
    body = build_request(c, value["context"], value["query"], value["max_tokens"], value["temperature"])
    try:
        raw = http_json("chat/completions", body, key, timeout, value["output_limit"])
    except urllib.error.HTTPError as exc:
        # Status is useful evidence; URLs, headers and error bodies may contain private data.
        integer(exc.code, "HTTP status", 400, 599)
        exc.close()
        return {"ok": False, "reason": "provider_http_error", "http_status": exc.code}
    # Return the receipt even when normalization refuses it: failed attempts are evidence.
    try:
        result = normalize_response(raw, c, value["max_tokens"])
        return {"ok": True, **_json_safe(result)}
    except (ValueError, TypeError, KeyError, InvalidOperation) as exc:
        usage = None
        try:
            usage = _provider_usage(raw, c, value["max_tokens"])
        except (ValueError, TypeError, KeyError):
            pass
        request_id = raw.get("id") if isinstance(raw, dict) else None
        if not isinstance(request_id, str) or not request_id.strip() or len(request_id) > 4096:
            request_id = None
        return {"ok": False, "reason": "provider_response_refused", "receipt": _json_safe(raw),
                "usage": usage, "request_id": request_id}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("worker")
    c = sub.add_parser("catalog")
    c.add_argument("--model", required=True)
    c.add_argument("--provider", required=True)
    c.add_argument("--out", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "worker":
            raw = sys.stdin.buffer.read(8 * 1024 * 1024 + 1)
            require(len(raw) <= 8 * 1024 * 1024, "worker_input_limit")
            print(json.dumps(worker(json.loads(raw)), allow_nan=False))
        else:
            # Metadata only; no model generation or credential artifacts. Never overwrite.
            require(not args.out.exists(), "output_exists")
            require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+:free", args.model), "exact_free_model_required")
            require(bool(os.environ.get("OPENROUTER_API_KEY")), "OPENROUTER_API_KEY_missing")
            data = http_json("models/" + urllib.parse.quote(args.model, safe="/") + "/endpoints", None, os.environ["OPENROUTER_API_KEY"], 15, 2 * 1024 * 1024)
            catalog = catalog_from_api(args.model, args.provider, data)
            with args.out.open("x", encoding="utf-8", newline="\n") as handle:
                handle.write(json.dumps(catalog, indent=2, allow_nan=False) + "\n")
        return 0
    except Exception:
        # Never echo HTTP bodies, credentials, submitted text or arbitrary provider errors.
        print("free-provider operation failed; no retry or fallback", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
