#!/usr/bin/env python3
"""Bounded local-process and free-provider boundary tests; no network/model calls."""
from __future__ import annotations

import copy
from datetime import datetime, timezone
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.error
from unittest.mock import patch

import contextdoctor as cd
from test_contextdoctor import H, manifest, encode, digest

try:
    import contextdoctor_run as runner
    import contextdoctor_openrouter as provider
except ModuleNotFoundError:
    runner = provider = None


def executable_manifest(root, adapter="container"):
    m = manifest()
    m["verifier"].update(kind="exact_match", digest=digest(cd.EXACT_VERIFIER))
    m["arms"]["candidate"]["transformation"] = "identity"
    settings = {"temperature": "0", "max_tokens": 32}
    m["pins"]["settings_hash"] = cd.canonical_digest(settings)
    m["execution"] = {"adapter": adapter, "image": H if adapter == "container" else None,
                      "agent_argv": ["/usr/bin/test-agent"], "tokenfold_argv": [],
                      "model": m["pins"]["model"], "provider": "synthetic", "settings": settings,
                      "catalog_hash": None}
    m["limits"]["max_operations"] = 30
    m["cost_policy"]["categories"]["infrastructure"]["allocation"] = "equal_arms"
    for t in m["tasks"]:
        for key, text in (("source_hash", "answer: YES"), ("prompt_hash", "Return only the answer."), ("reference_hash", "YES")):
            raw = text.encode()
            h = digest(raw)
            (root / (h[7:] + ".txt")).write_bytes(raw)
            t[key] = h
            m["bindings"]["artifacts"][h] = h[7:] + ".txt"
        t["snapshot_hash"] = t["source_hash"]
    m["verifier"]["spec_hash"] = cd.canonical_digest({"algorithm": cd.EXACT_VERIFIER,
        "references": {t["id"]: t["reference_hash"] for t in m["tasks"]}})
    m["content_hash"] = cd.canonical_digest(m)
    (root / "experiment.json").write_bytes(encode(m))
    return m


def approval(m):
    p = cd.build_plan(m)
    return {"schema_version": "1.0", "kind": "contextdoctor.approval",
            "manifest_hash": m["content_hash"], "schedule_hash": p["schedule_hash"],
            "execution_hash": p["execution_hash"], "implementation_hash": runner.implementation_hash(),
            "python_hash": digest(Path(sys.executable).read_bytes()),
            "expires_at": "2099-01-01T00:00:00Z", "adapter": m["execution"]["adapter"],
            "limits": copy.deepcopy(m["limits"]), "image": m["execution"]["image"],
            "docker_path": str(Path(sys.executable).resolve()), "docker_hash": digest(Path(sys.executable).read_bytes()),
            "external_data": "synthetic_only", "credential_env": None}


class Execution(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(runner, "bounded execution is not implemented")
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def test_approval_binds_scope_schedule_implementation_expiry_and_caps(self):
        m = executable_manifest(self.root)
        a = approval(m)
        runner.validate_approval(m, a)
        mutations = [lambda a: a.update(manifest_hash=H), lambda a: a.update(schedule_hash=H),
                     lambda a: a.update(implementation_hash=H), lambda a: a.update(python_hash=H),
                     lambda a: a.update(expires_at="2000-01-01T00:00:00Z"),
                     lambda a: a["limits"].update(max_operations=999), lambda a: a.update(image=digest("wrong image"))]
        for mutate in mutations:
            broken = copy.deepcopy(a)
            mutate(broken)
            with self.assertRaises(ValueError):
                runner.validate_approval(m, broken)

    def test_bounded_process_keeps_exact_output(self):
        result = runner.bounded_process([sys.executable, "-I", "-c", "import sys;sys.stdout.buffer.write(sys.stdin.buffer.read())"],
                                        b"hello", 3, 512)
        self.assertEqual(result, b"hello")

    def test_timeout_and_output_cap_stop_real_processes(self):
        for code, reason in (("import time;time.sleep(10)", "timeout"),
                             ("import sys;sys.stdout.write('x'*100000)", "output_limit"),
                             ("import sys;sys.stderr.write('x'*100000)", "output_limit"),
                             ("raise SystemExit(5)", "process_failed")):
            start = time.monotonic()
            with self.assertRaises(runner.ExecutionError) as raised:
                runner.bounded_process([sys.executable, "-I", "-c", code], b"", 0.3, 100)
            self.assertEqual(raised.exception.reason, reason)
            self.assertLess(time.monotonic() - start, 5)

    def test_dispatch_survives_failed_first_agent_without_retry_or_next_cell(self):
        m = executable_manifest(self.root)
        a = approval(m)
        (self.root / "approval.json").write_bytes(encode(a))
        calls = []
        def fail_agent(argv, payload, seconds, limit):
            calls.append(json.loads(payload))
            raise runner.ExecutionError("process_failed")
        with patch.object(runner.Container, "preflight"), patch.object(runner.Container, "invoke", side_effect=fail_agent):
            result = runner.run(self.root / "experiment.json", self.root / "approval.json", self.root / "out")
        self.assertEqual(result, 1)
        self.assertEqual(len(calls), 1)
        self.assertNotIn("reference", calls[0])
        r = cd.build_report(cd.load_experiment(self.root / "out/experiment.json", self.root / "out/episodes.jsonl"))
        self.assertEqual(r["counts"]["executed"], 1)
        self.assertEqual(r["counts"]["unexecuted"], 3)
        self.assertFalse(r["evidence_complete"])
        events = [json.loads(s) for s in (self.root / "out/operations.jsonl").read_text().splitlines()]
        self.assertEqual(sum(e["event"] == "dispatch" and e["role"] == "agent" for e in events), 1)

    def test_real_fake_agent_outputs_independently_verified_receipts(self):
        m = executable_manifest(self.root)
        (self.root / "approval.json").write_bytes(encode(approval(m)))
        def fake_agent(argv, payload, seconds, limit):
            return runner.bounded_process([sys.executable, "-I", "-c", "import sys,json;x=json.load(sys.stdin);print(x['context'].split(': ')[1],end='')"],
                                          payload, seconds, limit)
        with patch.object(runner.Container, "preflight"), patch.object(runner.Container, "invoke", side_effect=fake_agent):
            result = runner.run(self.root / "experiment.json", self.root / "approval.json", self.root / "out")
        self.assertEqual(result, 0)
        r = cd.build_report(cd.load_experiment(self.root / "out/experiment.json", self.root / "out/episodes.jsonl"))
        self.assertEqual(r["counts"]["verified_successes"], {"baseline": 2, "candidate": 2})
        self.assertEqual(r["exposure"]["candidate"]["no_op"], 2)
        self.assertIsNone(r["economics"]["provider_billed"]["baseline"]["cost_per_verified_success"])

    def test_container_command_is_networkless_readonly_nonroot_and_cleans_up(self):
        m = executable_manifest(self.root)
        c = runner.Container(approval(m), m["execution"])
        commands = []
        def execute(argv, payload, seconds, limit, **kw):
            commands.append(argv)
            if "run" in argv:
                for flag in ("--network=none", "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user=65534:65534", "--pull=never"):
                    self.assertIn(flag, argv)
                self.assertFalse(any("mount" in a or "volume" in a or "privileged" in a for a in argv))
                return b"YES"
            return b""
        with patch.object(runner, "bounded_process", side_effect=execute):
            self.assertEqual(c.invoke(["/bin/agent"], b"{}", 1, 100), b"YES")
        self.assertTrue(any("rm" in command and "--force" in command for command in commands))

    def test_plan_is_deterministic_and_does_not_import_executor(self):
        m = executable_manifest(self.root)
        self.assertEqual(cd.build_plan(m), cd.build_plan(m))
        self.assertFalse(cd.build_plan(m)["execution_permitted"])

    def test_executable_hashing_is_streamed_above_artifact_size_limit(self):
        path = self.root / "executable"
        with path.open("wb") as handle:
            handle.truncate(cd.MAX_ARTIFACT_BYTES + 1)
        self.assertEqual(runner.executable_hash(path), digest(path.read_bytes()))

    def test_tokenfold_receipt_controls_fallback_and_recovery_is_not_silently_lost(self):
        with patch.object(runner.Container, "invoke", return_value=(b"{}", b'{"schema_version":"2.0","status":"passthrough","bypass":{"reason":"gate"}}')):
            c = runner.Container({"docker_path": "unused", "image": H}, {})
            r = runner.tokenfold_transform(c, ["/bin/tokenfold", "compress", "-"], b"{}", 1, 8192)
            self.assertEqual(r["disposition"], "fallback")
        with patch.object(runner.Container, "invoke", return_value=(b'{"$tf_ref":"x"}', b'{"schema_version":"2.0","status":"compressed"}')):
            with self.assertRaises(ValueError):
                runner.tokenfold_transform(c, ["/bin/tokenfold", "compress", "-"], b"{}", 1, 8192)

    def test_cleanup_failure_does_not_count_as_confirmed_removal(self):
        m = executable_manifest(self.root)
        c = runner.Container(approval(m), m["execution"])
        def fail_cleanup(argv, *args, **kw):
            if "run" in argv:
                return b"ok"
            raise runner.ExecutionError("process_failed")
        with patch.object(runner, "bounded_process", side_effect=fail_cleanup):
            with self.assertRaises(runner.ExecutionError) as raised:
                c.invoke(["/bin/agent"], b"{}", 1, 100)
        self.assertEqual(raised.exception.reason, "container_cleanup_unconfirmed")

    def test_cap_before_episode_dispatch_leaves_all_cells_unexecuted(self):
        m = executable_manifest(self.root)
        m["limits"]["max_operations"] = 1
        m["content_hash"] = cd.canonical_digest(m)
        (self.root / "experiment.json").write_bytes(encode(m))
        (self.root / "approval.json").write_bytes(encode(approval(m)))
        with patch.object(runner.Container, "preflight"):
            result = runner.run(self.root / "experiment.json", self.root / "approval.json", self.root / "out")
        self.assertEqual(result, 1)
        r = cd.build_report(cd.load_experiment(self.root / "out/experiment.json", self.root / "out/episodes.jsonl"))
        self.assertEqual(r["counts"]["executed"], 0)
        self.assertEqual(r["counts"]["unexecuted"], 4)

    def test_real_timeout_and_overflow_through_container_stop_cleanup_and_next_cell(self):
        real_process = runner.bounded_process
        for index, code in enumerate(("import time;time.sleep(10)", "import sys;sys.stdout.write('x'*100000)")):
            root = self.root / str(index)
            root.mkdir()
            m = executable_manifest(root)
            m["limits"]["operation_seconds"] = "0.3"
            m["content_hash"] = cd.canonical_digest(m)
            (root / "experiment.json").write_bytes(encode(m))
            (root / "approval.json").write_bytes(encode(approval(m)))
            commands = []
            def transport(argv, payload, seconds, limit, **kwargs):
                commands.append(argv)
                helper = code if "run" in argv else "pass"
                return real_process([sys.executable, "-I", "-c", helper], payload, seconds, limit, **kwargs)
            with patch.object(runner.Container, "preflight"), patch.object(runner, "bounded_process", side_effect=transport):
                self.assertEqual(runner.run(root / "experiment.json", root / "approval.json", root / "out"), 1)
            self.assertEqual(sum("run" in command for command in commands), 1)
            self.assertTrue(any("rm" in command and "--force" in command for command in commands))
            r = cd.build_report(cd.load_experiment(root / "out/experiment.json", root / "out/episodes.jsonl"))
            self.assertEqual(r["counts"]["executed"], 1)
            self.assertEqual(r["counts"]["unexecuted"], 3)
            self.assertFalse(r["evidence_complete"])

    @unittest.skipUnless(os.environ.get("CONTEXTDOCTOR_TEST_IMAGE") and os.environ.get("CONTEXTDOCTOR_DOCKER_PATH"),
                         "opt-in approved Docker fixture image/executable not supplied")
    def test_opt_in_actual_docker_boundary(self):
        image = os.environ["CONTEXTDOCTOR_TEST_IMAGE"]
        cd.hash_value(image, "approved test image")
        docker = Path(os.environ["CONTEXTDOCTOR_DOCKER_PATH"])
        self.assertTrue(docker.is_absolute() and docker.is_file())
        container = runner.Container({"docker_path": str(docker), "image": image}, {})
        container.preflight(5)
        code = "import json,os,socket,sys;assert os.geteuid()==65534;assert 'OPENROUTER_API_KEY' not in os.environ;x=json.load(sys.stdin);print(x['context'],end='')"
        self.assertEqual(container.invoke(["/usr/bin/python3", "-c", code], b'{"context":"YES","query":"synthetic"}', 5, 8192), b"YES")
        code = """import os, socket
try:
    open('/contextdoctor-write-probe', 'w')
except OSError:
    pass
else:
    raise AssertionError('writable root filesystem')
s = socket.socket()
s.settimeout(1)
try:
    s.connect(('1.1.1.1', 53))
except OSError:
    pass
else:
    raise AssertionError('external network available')
print('contained', end='')
"""
        self.assertEqual(container.invoke(["/usr/bin/python3", "-c", code], b"", 5, 8192), b"contained")
        before = runner.bounded_process(container.base + ["ps", "--all", "--filter", "name=contextdoctor-", "--format", "{{.ID}}"], b"", 5, 8192)
        code = "import subprocess,time;subprocess.Popen(['/usr/bin/python3','-c','import time;time.sleep(60)']);time.sleep(60)"
        with self.assertRaises(runner.ExecutionError) as raised:
            container.invoke(["/usr/bin/python3", "-c", code], b"", 1, 8192)
        self.assertEqual(raised.exception.reason, "timeout")
        after = runner.bounded_process(container.base + ["ps", "--all", "--filter", "name=contextdoctor-", "--format", "{{.ID}}"], b"", 5, 8192)
        self.assertEqual(after, before, "timed-out container or descendants remain")


def catalog():
    return {"schema_version": "1.0", "kind": "contextdoctor.openrouter_catalog",
            "model": "example/model:free", "canonical_model": "example/model", "provider": "test/free",
            "pricing": {"prompt": "0", "completion": "0", "request": "0"},
            "supported_parameters": ["max_tokens", "temperature"]}


def response():
    return {"id": "gen-test", "model": "example/model", "provider": "test/free",
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "YES"}}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 2, "total_tokens": 14,
                      "prompt_tokens_details": {"cached_tokens": 4, "cache_write_tokens": 0}, "cost": 0}}


def free_manifest(root):
    m = executable_manifest(root, "openrouter_free")
    c = catalog()
    raw = encode(c)
    h = digest(raw)
    (root / "catalog.json").write_bytes(raw)
    m["bindings"]["artifacts"][h] = "catalog.json"
    m["pins"]["model"] = c["model"]
    m["execution"].update(model=c["model"], provider=c["provider"], catalog_hash=h)
    m["cost_policy"]["categories"]["model"]["basis"] = "usage_priced"
    m["cost_policy"]["rates"] = [{"model": c["model"], "metric": k, "rate": "0", "per": 1,
                                  "currency": "USD", "evidence_hash": h} for k in cd.METRICS]
    m["content_hash"] = cd.canonical_digest(m)
    (root / "experiment.json").write_bytes(encode(m))
    a = approval(m)
    a["credential_env"] = "OPENROUTER_API_KEY"
    (root / "approval.json").write_bytes(encode(a))
    return m, a


class FreeProvider(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(provider, "free-only provider adapter is not implemented")

    def test_free_request_pins_provider_and_zero_prices_without_fallbacks_or_tools(self):
        body = provider.build_request(catalog(), "context", "query", 32, "0")
        self.assertEqual(body["model"], "example/model:free")
        self.assertEqual(body["provider"]["only"], ["test/free"])
        self.assertFalse(body["provider"]["allow_fallbacks"])
        self.assertEqual(body["provider"]["max_price"], {"prompt": 0, "completion": 0, "request": 0, "image": 0})
        self.assertNotIn("tools", body)
        self.assertNotIn("models", body)
        self.assertEqual(body["max_tokens"], 32)

    def test_paid_router_unknown_rates_and_nonzero_pricing_are_refused(self):
        for mutate in (lambda c: c.update(model="example/paid"), lambda c: c.update(model="openrouter/free"),
                       lambda c: c["pricing"].update(prompt="0.001"), lambda c: c["pricing"].pop("completion"),
                       lambda c: c["pricing"].update(unknown_fee="0")):
            c = catalog()
            mutate(c)
            with self.assertRaises(ValueError):
                provider.build_request(c, "context", "query", 32, "0")

    def test_live_catalog_zero_discount_is_metadata_and_missing_request_price_stays_missing(self):
        raw = {"data": {"id": "example/model:free", "endpoints": [{"tag": "test/free",
               "pricing": {"prompt": "0", "completion": "0", "discount": 0},
               "supported_parameters": ["max_tokens", "temperature"]}]}}
        c = provider.catalog_from_api("example/model:free", "test/free", raw)
        self.assertEqual(c["pricing"], {"prompt": "0", "completion": "0"})
        self.assertEqual(provider.build_request(c, "context", "query", 32, "0")["provider"]["max_price"]["request"], 0)
        self.assertEqual(provider.normalize_response(response(), c, 32)["cost_coverage_reason"], "request_price_unreported")
        c["pricing"]["request"] = "0"
        self.assertEqual(provider.normalize_response(response(), c, 32)["cost_coverage_reason"], "cache_price_unreported")
        c["pricing"]["input_cache_read"] = "0"
        self.assertIsNone(provider.normalize_response(response(), c, 32)["cost_coverage_reason"])
        raw["data"]["endpoints"][0]["pricing"]["discount"] = "0.1"
        with self.assertRaises(ValueError):
            provider.catalog_from_api("example/model:free", "test/free", raw)

    def test_overlap_is_subtracted_once_and_missing_details_are_unknown(self):
        r = provider.normalize_response(response(), catalog(), 32)
        self.assertEqual(r["usage"]["snapshots"][0], {"input": 8, "cache_read": 4, "cache_write": 0, "output": 2})
        self.assertEqual(r["usage"]["source"], "provider_observed")
        raw = response()
        raw["usage"].pop("prompt_tokens_details")
        self.assertIsNone(provider.normalize_response(raw, catalog(), 32)["usage"])

    def test_incomplete_text_is_not_accepted_as_a_final_answer(self):
        value = {"action": "request", "catalog": catalog(), "context": "synthetic", "query": "answer",
                 "max_tokens": 32, "temperature": "0", "timeout": "2", "output_limit": 8192}
        for finish in ("length", "content_filter"):
            raw = response()
            raw["choices"][0]["finish_reason"] = finish
            with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(provider, "http_json", return_value=raw):
                result = provider.worker(value)
            self.assertFalse(result["ok"])
            self.assertNotIn("output", result)
            self.assertEqual(result["usage"]["snapshots"][0]["output"], 2)
            self.assertEqual(result["request_id"], raw["id"])

    def test_wrong_model_tools_nonzero_cost_and_invalid_usage_stop(self):
        for mutate in (lambda r: r.update(model="other/model"), lambda r: r["usage"].update(cost=1),
                       lambda r: r["usage"].update(prompt_tokens=True),
                       lambda r: r["choices"][0]["message"].update(tool_calls=[{}]),
                       lambda r: r["usage"].update(completion_tokens=33),
                       lambda r: r["usage"]["prompt_tokens_details"].update(cached_tokens=20)):
            r = response()
            mutate(r)
            with self.assertRaises(ValueError):
                provider.normalize_response(r, catalog(), 32)

    def test_redirects_are_rejected(self):
        with self.assertRaises(ValueError):
            provider.NoRedirect().redirect_request(None, None, 302, "redirect", {}, "https://example.com")

    def test_http_failure_preserves_only_safe_status_without_retry(self):
        value = {"action": "request", "catalog": catalog(), "context": "synthetic", "query": "answer",
                 "max_tokens": 32, "temperature": "0", "timeout": "2", "output_limit": 8192}
        error = urllib.error.HTTPError("https://example.invalid/private", 429, "private detail", {}, io.BytesIO(b"private body"))
        with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(provider, "http_json", side_effect=error) as call:
            result = provider.worker(value)
        self.assertEqual(result, {"ok": False, "reason": "provider_http_error", "http_status": 429})
        self.assertEqual(call.call_count, 1)
        self.assertNotIn("private", json.dumps(result))

    def test_http_failure_stops_scope_and_retains_status_in_execution(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            free_manifest(root)
            def transport(argv, raw, seconds, limit, **kwargs):
                if json.loads(raw)["action"] == "preflight":
                    return json.dumps({"catalog": catalog()}).encode()
                return b'{"ok":false,"reason":"provider_http_error","http_status":429}'
            with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(runner, "bounded_process", side_effect=transport) as call:
                self.assertEqual(runner.run(root / "experiment.json", root / "approval.json", root / "out"), 1)
            self.assertEqual(call.call_count, 2)
            execution = json.loads((root / "out/execution.json").read_text())
            self.assertEqual(execution["stopped_reason"], "provider_http_429")
            report = cd.build_report(cd.load_experiment(root / "out/experiment.json", root / "out/episodes.jsonl"))
            self.assertEqual(report["counts"]["unexecuted"], 3)
            self.assertFalse(report["evidence_complete"])

    def test_failed_provider_receipt_is_retained_without_retry(self):
        r = response()
        r["usage"]["cost"] = 1
        calls = []
        def transport(*args):
            calls.append(args)
            return r
        value = {"action": "request", "catalog": catalog(), "context": "synthetic", "query": "answer",
                 "max_tokens": 32, "temperature": "0", "timeout": "2", "output_limit": 8192}
        with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(provider, "http_json", side_effect=transport):
            result = provider.worker(value)
        self.assertFalse(result["ok"])
        self.assertEqual(result["receipt"]["usage"]["cost"], 1)
        self.assertEqual(len(calls), 1)

    def test_truncated_missing_answer_retains_valid_provider_usage_without_success(self):
        raw = response()
        raw["choices"][0].update(finish_reason="length")
        raw["choices"][0]["message"]["content"] = None
        value = {"action": "request", "catalog": catalog(), "context": "synthetic", "query": "answer",
                 "max_tokens": 32, "temperature": "0", "timeout": "2", "output_limit": 8192}
        with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(provider, "http_json", return_value=raw):
            result = provider.worker(value)
        self.assertFalse(result["ok"])
        self.assertNotIn("output", result)
        self.assertEqual(result["usage"]["snapshots"][0], {"input": 8, "cache_read": 4, "cache_write": 0, "output": 2})
        raw["model"] = "other/model"
        with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(provider, "http_json", return_value=raw):
            self.assertIsNone(provider.worker(value)["usage"])

    def test_failed_attempt_usage_reaches_report_without_becoming_verified_success(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            free_manifest(root)
            raw = response()
            raw["usage"].pop("cost")
            raw["choices"][0].update(finish_reason="length")
            raw["choices"][0]["message"]["content"] = None
            def transport(argv, payload, seconds, limit, **kwargs):
                value = json.loads(payload)
                if value["action"] == "preflight":
                    return json.dumps({"catalog": catalog()}).encode()
                return json.dumps(provider.worker(value)).encode()
            with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(provider, "http_json", return_value=raw), patch.object(runner, "bounded_process", side_effect=transport):
                self.assertEqual(runner.run(root / "experiment.json", root / "approval.json", root / "out"), 1)
            report = cd.build_report(cd.load_experiment(root / "out/experiment.json", root / "out/episodes.jsonl"))
            events = [json.loads(line) for line in (root / "out/operations.jsonl").read_text().splitlines()]
            agent_terminal = next(event for event in events if event["role"] == "agent" and event["event"] == "terminal")
            self.assertEqual(agent_terminal["request_id"], "gen-test")
            self.assertEqual(report["secondary"]["baseline"]["provider_tokens"], {"input": 8, "cache_read": 4, "cache_write": 0, "output": 2})
            self.assertEqual(report["counts"]["verified_successes"], {"baseline": 0, "candidate": 0})
            self.assertEqual(report["counts"]["unexecuted"], 3)
            self.assertFalse(report["evidence_complete"])

    def test_transport_does_not_inherit_user_home_or_config(self):
        result = runner.bounded_process([sys.executable, "-I", "-c", "import os,json;print(json.dumps([os.environ.get('HOME'),os.environ.get('USERPROFILE'),os.environ.get('SECRET_TEST')]))"],
                                        b"", 3, 8192, env={})
        home, profile, secret = json.loads(result)
        self.assertIsNotNone(home)
        self.assertEqual(home, profile)
        self.assertNotEqual(home, os.environ.get("USERPROFILE"))
        self.assertIsNone(secret)

    def test_free_scope_cannot_reprice_zero_catalog_as_nonzero_spend(self):
        with tempfile.TemporaryDirectory() as tmp:
            m, a = free_manifest(Path(tmp))
            runner.validate_approval(m, a)
            m["cost_policy"]["rates"][0]["rate"] = "1"
            m["content_hash"] = cd.canonical_digest(m)
            a = approval(m)
            a["credential_env"] = "OPENROUTER_API_KEY"
            with self.assertRaises(ValueError):
                runner.validate_approval(m, a)

    def test_unexpected_provider_charge_survives_failed_attempt_in_all_attempt_ledger(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            free_manifest(root)
            def transport(argv, raw, seconds, limit, **kwargs):
                payload = json.loads(raw)
                if payload["action"] == "preflight":
                    return json.dumps({"catalog": catalog()}).encode()
                return b'{"ok":false,"receipt":{"id":"gen-failed","usage":{"cost":"1"}}}'
            with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(runner, "bounded_process", side_effect=transport):
                self.assertEqual(runner.run(root / "experiment.json", root / "approval.json", root / "out"), 1)
            r = cd.build_report(cd.load_experiment(root / "out/experiment.json", root / "out/episodes.jsonl"))
            self.assertEqual(r["accounting"]["baseline"]["provider_billed"]["known_subtotal"], "1")
            self.assertEqual(r["counts"]["unexecuted"], 3)
            self.assertFalse(r["evidence_complete"])

    def test_missing_request_price_keeps_token_subtotal_but_blocks_complete_economics(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            m, a = free_manifest(root)
            c = catalog()
            c["pricing"].pop("request")
            raw = encode(c)
            h = digest(raw)
            (root / "catalog-optional-request.json").write_bytes(raw)
            m["bindings"]["artifacts"][h] = "catalog-optional-request.json"
            m["execution"]["catalog_hash"] = h
            for rate in m["cost_policy"]["rates"]:
                rate["evidence_hash"] = h
            m["content_hash"] = cd.canonical_digest(m)
            (root / "experiment.json").write_bytes(encode(m))
            a = approval(m)
            a["credential_env"] = "OPENROUTER_API_KEY"
            (root / "approval.json").write_bytes(encode(a))
            def transport(argv, raw, seconds, limit, **kwargs):
                payload = json.loads(raw)
                if payload["action"] == "preflight":
                    return json.dumps({"catalog": c}).encode()
                return json.dumps({"ok": True, **provider.normalize_response(response(), c, 32)}).encode()
            with patch.dict(os.environ, {"OPENROUTER_API_KEY": "test-not-a-real-key"}), patch.object(runner, "bounded_process", side_effect=transport):
                self.assertEqual(runner.run(root / "experiment.json", root / "approval.json", root / "out"), 0)
            r = cd.build_report(cd.load_experiment(root / "out/experiment.json", root / "out/episodes.jsonl"))
            self.assertEqual(r["accounting"]["baseline"]["usage_priced"]["known_subtotal"], "0")
            self.assertFalse(r["evidence_complete"])
            self.assertTrue(any(item["reason"] == "request_price_unreported" for item in r["missing_costs"]))
            self.assertIsNone(r["economics"]["usage_priced"]["baseline"]["cost_per_verified_success"])


if __name__ == "__main__":
    unittest.main()
