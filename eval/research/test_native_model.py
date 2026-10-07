"""Offline resident-client contracts, not model-quality or weight-attestation evidence."""
import json
import subprocess
import threading
import tempfile
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from unittest.mock import patch

import native_model as nm


class NativeTests(unittest.TestCase):
    def test_opt_in_preflight_seals_body_matches_usage_and_retains_failed_accounting(self):
        requests=[]
        messages=[{"role":"user","content":"x"*5000}]
        mismatch=False; malformed=False
        def transport(port,path,body,timeout):
            requests.append(path)
            if path=="models":return {"data":[{"id":"Qwen/Qwen3.5-0.8B"}]}
            if path=="apply-template":
                messages[0]["content"]="caller changed after sealing"
                self.assertEqual(body["messages"][0]["content"],"x"*5000)
                return {"prompt":"template"}
            if path=="tokenize":return {"tokens":[True] if malformed else [1]*20}
            self.assertEqual(body["messages"][0]["content"],"x"*5000)
            return self.reply(usage={"prompt_tokens":21 if mismatch else 20,"completion_tokens":4})
        for mismatch,malformed in ((False,False),(True,False),(False,True)):
            messages[0]["content"]="x"*5000
            model=nm.NativeModel("Qwen/Qwen3.5-0.8B","a"*64,8000,2,25,128,2048,transport,
                                 native_template_preflight=True)
            if mismatch or malformed:
                with self.assertRaises(nm.ModelError): model.chat(messages,0)
                before=len(requests)
                with self.assertRaises(nm.ModelError): model.chat(messages,0)
                self.assertEqual(len(requests),before)
            else:self.assertEqual(model.chat(messages,0),"answer")
            self.assertEqual(len(model.accounting_calls),1)
            self.assertEqual(len(model.calls),0 if malformed else 1)
            self.assertIsNone(model.accounting_calls[0]["cost"])
            if mismatch:self.assertEqual(model.calls[0]["usage"]["input_tokens"],21)
        default,_=self.model([])
        with self.assertRaises(nm.ModelError):default.chat([{"role":"user","content":"x"*17000}],0)
        self.assertEqual(default.accounting_calls,[])  # Legacy byte preflight stays the default.
        def oversized(port,path,body,timeout):
            if path=="models":return {"data":[{"id":"Qwen/Qwen3.5-0.8B"}]}
            if path=="apply-template":return {"prompt":"template"}
            if path=="tokenize":return {"tokens":[1]*1000}
            raise AssertionError("over-budget generation must not run")
        big=nm.NativeModel("Qwen/Qwen3.5-0.8B","a"*64,8000,1,25,128,2048,oversized,native_template_preflight=True)
        with self.assertRaisesRegex(nm.ModelError,"context_allowance"):
            big.chat([{"role":"user","content":"input"}],0)
        self.assertFalse(big.calls)
        self.assertEqual(big.accounting_calls[0]["reason"],"context_allowance")

    def test_template_accounting_is_explicit_preserves_roles_and_refuses_bad_counts(self):
        body={"messages":[{"role":"system","content":"rule"},{"role":"user","content":"input"}],
              "chat_template_kwargs":{"enable_thinking":False}}
        calls=[]
        def transport(port,path,payload,timeout):
            calls.append((path,payload))
            return {"prompt":"<templated>"} if path=="apply-template" else {"tokens":[1,2,3]}
        result=nm.template_accounting(8000,body,transport=transport)
        self.assertEqual(result["prompt_tokens"],3)
        self.assertIs(calls[0][1],body)
        self.assertEqual(calls[1][1],{"content":"<templated>","add_special":True,"parse_special":True})
        for tokens in ([],[True],[-1],["1"]):
            with self.assertRaises(nm.ModelError):
                nm.template_accounting(8000,body,transport=lambda port,path,payload,timeout:
                    {"prompt":"template"} if path=="apply-template" else {"tokens":tokens})
        with self.assertRaises(nm.ModelError):
            nm.template_accounting(8000,body,transport=lambda *args:{"prompt":[]})

    def test_independent_profile_refuses_malformed_contract_before_tasks_or_models(self):
        import acon_benchmark as ab
        with tempfile.TemporaryDirectory() as directory:
            profile=Path(directory)/"profile.json"
            args=["--run-live","--acon-root","unused","--provider","native","--model","Qwen/Qwen3.5-4B",
                  "--output-dir","unused","--generative-arm","--generative-model-profile",str(profile)]
            for data in (b"{}",b'{"name":"first","name":"second"}',b"x"*65537):
                profile.write_bytes(data)
                with patch.object(ab.rp,"load_tasks") as tasks,patch.object(nm,"NativeModel") as initialize:
                    with self.assertRaises((SystemExit,ValueError)):
                        ab.main(args)
                    tasks.assert_not_called(); initialize.assert_not_called()
            with patch.object(ab.rp,"load_tasks") as tasks:
                unsupported=list(args); unsupported[unsupported.index("--provider")+1]="ollama"
                with self.assertRaises(SystemExit): ab.main(unsupported)
                tasks.assert_not_called()
            config={"name":"Qwen/Qwen3.5-0.8B","digest":"b"*64,"port":8001,"max_calls":1,
                    "timeout":25,"output_tokens":1024,"context_tokens":16384,"native_template_preflight":"true"}
            profile.write_text(json.dumps(config))
            with patch.object(ab.rp,"load_tasks") as tasks,patch.object(nm,"NativeModel") as initialize:
                with self.assertRaises(SystemExit):ab.main(args)
                tasks.assert_not_called();initialize.assert_not_called()
            config["native_template_preflight"]=True;profile.write_text(json.dumps(config))
            with patch.object(ab.rp,"load_tasks") as tasks,patch.object(nm,"NativeModel") as initialize:
                with self.assertRaises(SystemExit):ab.main(args+["--generative-cli-approval","unused"])
                tasks.assert_not_called();initialize.assert_not_called()

    def test_independent_native_summarizer_profile_keeps_answering_and_usage_separate(self):
        import acon_benchmark as ab
        import generate_observation as gen
        import hashlib
        import freeze_campaign as fc
        repository=Path(ab.__file__).resolve().parents[2]
        for cli, mutate in ((False,False),(False,True),(True,False),(True,True)):
            with tempfile.TemporaryDirectory(dir=repository) as directory:
                root=Path(directory); tasks=root/"tasks"; tasks.mkdir()
                (tasks/"task.json").write_text(json.dumps({"id":"task","family":"fixture","tier":"A",
                    "source":"answer "*20,"query":"What?","gold_answer":"answer","critical_atoms":[]}))
                binary=root/"binary"; binary.write_bytes(b"scripted executable")
                profile=root/"summarizer.json"
                profile.write_text(json.dumps({"name":"Qwen/Qwen3.5-0.8B","digest":"b"*64,"port":8001,
                    "max_calls":1,"timeout":25,"output_tokens":1024,"context_tokens":16384}))
                approval=root/"approval.json"
                approval.write_text(json.dumps({"model_revision":"Qwen/Qwen3.5-0.8B",
                    "arguments":["--backend","openai","--port","8001","--max-output-tokens","1024"]}))
                protocol=root/"protocol.json"; frozen=root/"frozen.json"
                protocol.write_text(json.dumps({"version":1,"scope":"smoke",
                    "arms":["raw","tokenfold-lossless","acon-observation","tokenfold-generative"],
                    "models":{"answering_and_compressor":"Qwen/Qwen3.5-4B@"+"a"*64,
                              "summarizer":"Qwen/Qwen3.5-0.8B@"+"b"*64},"seeds":[0],
                    "runtime":{"acon_revision":ab.ACON_REVISION,"transport":"native-openai-loopback-killable-subprocess",
                        "answering_roles":["system","user"],"assistant_prefill":False,"scorer_guideline":None,
                        "generative_guideline":gen.GUIDELINE,"headroom_revision":None,"native_loopback_port":8000,
                        "generative_model_profile_sha256":hashlib.sha256(profile.read_bytes()).hexdigest()},
                    "budgets":{"context_tokens":16384,"output_tokens":512,"max_calls":5,"timeout_seconds":25,"ratio":.5},
                    "quality":{"max_cfr":.01,"max_success_loss":.01,"confidence":.95,"min_raw_success_clusters":299},
                    "suites":[{"split":"test","workload":"json","path":tasks.relative_to(repository).as_posix()}]}))
                if cli:
                    config=json.loads(protocol.read_bytes())
                    config["arms"][-1]="tokenfold-generative-cli"
                    config["runtime"].update(generative_guideline=None,
                        generative_cli_approval_sha256=hashlib.sha256(approval.read_bytes()).hexdigest(),
                        generative_cli_guideline="tokenfold-native-cli-summarize-v1",generative_cli_timeout_ms=25000,
                        generative_cli_seed_control="worker-owned-not-set-by-benchmark")
                    config["runtime"]["native_template_preflight"]=True
                    protocol.write_text(json.dumps(config))
                frozen.write_text(json.dumps(fc.freeze(protocol,repository)))
                original=nm.NativeModel; models=[]
                def factory(*args,**kwargs):
                    name=kwargs.get("name",args[0] if args else None)
                    def transport(port,path,body,timeout):
                        if path=="models": return {"data":[{"id":name}]}
                        if path=="apply-template":return {"prompt":"template"}
                        if path=="tokenize":return {"tokens":[1]*20}
                        self.assertNotIn("gold_answer",json.dumps(body))
                        return self.reply(model=name)
                    model=original(*args,**kwargs,transport=transport); models.append(model); return model
                class Summary:
                    def __init__(self,model,*args,**kwargs): self.model=model; self.last_receipt=None
                    def __call__(self,task,target,seed):
                        self.model.chat([{"role":"user","content":task["source"]}],seed)
                        self.last_receipt={"disposition":"fell_back","valid_attempt":True,"reason":"over_budget"}
                        return task["source"]
                def acon(path,model):
                    return (lambda task,seed:model.chat([{"role":"user","content":task["source"]}],seed),
                            {"revision":ab.ACON_REVISION,"source_and_prompt_sha256":{}})
                original_run=ab.run_pilot
                def run(*args,**kwargs):
                    result=original_run(*args,**kwargs)
                    if mutate: profile.write_text("{}")
                    return result
                def worker(args,**kwargs):
                    self.assertNotIn("gold_answer",kwargs["input"])
                    Path(args[-1]).write_text(json.dumps({"disposition":"generated_unverified","runtime_invoked":True,
                        "inference_usage":{"input_tokens":25,"output_tokens":9},"wall_ms":3}))
                    return subprocess.CompletedProcess(args,0,stdout="answer",stderr="")
                args=["--run-live","--acon-root",str(root),"--provider","native","--model","Qwen/Qwen3.5-4B",
                      "--model-digest","a"*64,"--context-tokens","16384","--max-calls","5","--timeout","25",
                      "--tasks-dir",str(tasks),"--output-dir",str(root/"out"),"--generative-arm",
                      "--generative-model-profile",str(profile),"--campaign-protocol",str(protocol),
                      "--campaign-freeze",str(frozen)]
                if cli:
                    args.remove("--generative-arm")
                    args.extend(["--generative-cli-approval",str(approval)])
                    args.append("--native-template-preflight")
                    approved_bytes=approval.read_bytes()
                    bad_approval=json.loads(approved_bytes)
                    bad_approval["arguments"][3]="8002"
                    approval.write_text(json.dumps(bad_approval))
                    with patch.object(nm,"NativeModel") as initialize,patch.object(gen,"native_guard"),\
                         patch.object(ab.rp.rb,"_TOKENFOLD_BIN",str(binary)):
                        with self.assertRaises(SystemExit): ab.main(args)
                        initialize.assert_not_called()
                    approval.write_bytes(approved_bytes)
                with patch.object(nm,"NativeModel",side_effect=factory),patch.object(ab,"load_acon",side_effect=acon),\
                     patch.object(gen,"native_guard"),patch.object(gen,"GenerativeArm",Summary),\
                     patch.object(gen.subprocess,"run",side_effect=worker),\
                     patch.object(ab,"run_pilot",side_effect=run),patch.object(ab.rp.rb,"_TOKENFOLD_BIN",str(binary)),\
                     patch.object(ab.rp.rb,"TOKENIZER",{"is_exact":True}),patch.object(ab.rp.rb,"count_tokens",len),\
                     patch.object(ab.rp.rb,"compress_tokenfold",side_effect=lambda source,target:source):
                    self.assertEqual(ab.main(args),int(mutate))
                self.assertEqual([len(m.calls) for m in models],[5,1])
                if cli:self.assertEqual(len(models[0].accounting_calls),5)
                report=json.loads((root/"out/report.json").read_bytes())
                self.assertTrue(report["campaign_unchanged"])
                self.assertEqual(report["native_alias_unchanged"],not mutate)
                rows=json.loads((root/"out/economics.json").read_bytes())
                summary=next(r for r in rows if r["candidate"]==("tokenfold-generative-cli" if cli else "tokenfold-generative"))
                self.assertIsNone(summary["total_usage"])
                self.assertEqual(len(summary["usage_by_model"]),2)
                self.assertEqual(len(summary["compression_model_calls"]),1)

    def model(self, replies):
        requests = []

        def transport(port, path, body, timeout):
            requests.append((port, path, body))
            return {"data": [{"id": "Qwen/Qwen3.5-0.8B"}]} if path == "models" else replies.pop(0)

        model = nm.NativeModel("Qwen/Qwen3.5-0.8B", "a" * 64, 8000, 2, 25, 128, 16384, transport)
        return model, requests

    def reply(self, **changes):
        return {"model": "Qwen/Qwen3.5-0.8B", "usage": {"prompt_tokens": 20, "completion_tokens": 4},
                "choices": [{"finish_reason": "stop", "message": {"content": "answer"}}], **changes}

    def test_fresh_calls_non_thinking_schema_usage_and_no_zero_cost(self):
        model, requests = self.model([self.reply(), self.reply()])
        messages = [{"role": "user", "content": "same input"}]
        self.assertEqual(model.chat(messages, 0), "answer")
        self.assertEqual(model.chat(messages, 0, response_schema={"type": "object"}), "answer")
        self.assertEqual(len(model.calls), 2)
        self.assertEqual(model.calls[0]["usage"], {"input_tokens": 20, "output_tokens": 4})
        self.assertIsNone(model.calls[0]["cost"])
        with self.assertRaises(nm.ModelError):
            model.chat(messages, 0)
        bodies = [body for _, path, body in requests if path == "chat/completions"]
        self.assertEqual(len(bodies), 2)  # No answer caching or loading model weights in client.
        self.assertFalse(bodies[0]["chat_template_kwargs"]["enable_thinking"])
        self.assertTrue(bodies[1]["response_format"]["json_schema"]["strict"])

    def test_rejected_replies_preserve_usage_and_stop_without_text_export(self):
        for reply, reason in ((self.reply(model="other"),"model_alias_mismatch"),
                             (self.reply(choices=[]),"invalid_response"),
                             (self.reply(choices=[{"finish_reason":"length", "message":{"content":"PRIVATE_REJECTED"}}]),
                              "incomplete_response")):
            model, requests = self.model([reply])
            with self.assertRaises(nm.ModelError):
                model.chat([{"role": "user", "content": "input"}], 0)
            self.assertEqual(model.calls[0]["usage"]["output_tokens"], 4)
            self.assertEqual(model.calls[0]["reason"],reason)
            self.assertNotIn("PRIVATE", json.dumps(model.calls))
            before = len(requests)
            with self.assertRaises(nm.ModelError):
                model.chat([{"role": "user", "content": "input"}], 0)
            self.assertEqual(len(requests), before)
        model, _ = self.model([self.reply(usage={"prompt_tokens": True, "completion_tokens": 4})])
        model.chat([{"role": "user", "content": "input"}], 0)
        self.assertIsNone(model.calls[0]["usage"])
        model,_=self.model([])
        def private_error(port,path,body,timeout):
            if path=="models":return {"data":[{"id":model.name}]}
            raise nm.ModelError("PRIVATE_SOURCE_DIAGNOSTIC")
        model.transport=private_error
        with self.assertRaisesRegex(nm.ModelError,"generation_failed"):
            model.chat([{"role":"user","content":"input"}],0)
        self.assertNotIn("PRIVATE",json.dumps(model.calls))

    def test_invalid_input_and_alias_change_refuse_before_generation(self):
        model, requests = self.model([])
        for messages in ([], [{"role": "tool", "content": "input"}], [{"role": "user", "content": "x" * 17000}]):
            with self.assertRaises(nm.ModelError):
                model.chat(messages, 0)
        self.assertEqual(len(model.calls), 0)
        model.transport = lambda *args: {"data": [{"id": "other"}]}
        with self.assertRaises(nm.ModelError):
            model.chat([{"role": "user", "content": "input"}], 0)
        self.assertEqual(len(model.calls), 0)

    def test_reported_allowance_overruns_retain_usage_and_halt_without_retries(self):
        for usage, reason in (({"prompt_tokens": 20, "completion_tokens": 129}, "output_allowance_exceeded"),
                              ({"prompt_tokens": 16381, "completion_tokens": 4}, "context_allowance_exceeded")):
            model, requests = self.model([self.reply(usage=usage, choices=[{
                "finish_reason": "stop", "message": {"content": "REJECTED_OUTPUT"}}])])
            with self.assertRaisesRegex(nm.ModelError, reason):
                model.chat([{"role": "user", "content": "input"}], 0)
            self.assertEqual(model.calls[0]["usage"], {"input_tokens": usage["prompt_tokens"],
                                                     "output_tokens": usage["completion_tokens"]})
            self.assertEqual(model.calls[0]["reason"], reason)
            self.assertIsNone(model.calls[0]["cost"])
            self.assertNotIn("REJECTED_OUTPUT", json.dumps(model.calls))
            before = len(requests)
            with self.assertRaisesRegex(nm.ModelError, reason):
                model.chat([{"role": "user", "content": "input"}], 0)
            self.assertEqual(len(requests), before)
        model, _ = self.model([self.reply(usage={"prompt_tokens": 16256, "completion_tokens": 128})])
        self.assertEqual(model.chat([{"role": "user", "content": "input"}], 0), "answer")

    def test_killable_client_deadline_and_loopback_route_restriction(self):
        with patch.object(nm.subprocess, "run", side_effect=subprocess.TimeoutExpired("worker", 1)):
            with self.assertRaisesRegex(nm.ModelError, "client_deadline"):
                nm.bounded_request(8000, "models", None, 1)
        for port, path in ((True, "models"), (0, "models"), (8000, "https://example.com")):
            with self.assertRaises(nm.ModelError):
                nm.direct_request(port, path, None, 1)
        with self.assertRaises(nm.ModelError):
            json.loads('{"a":1,"a":2}', object_pairs_hook=nm.unique_object)

    def test_real_loopback_worker_reuses_server_but_generates_each_time(self):
        replies, request_paths = self.reply(), []

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def handle(self):
                try:
                    super().handle()
                except (BrokenPipeError, ConnectionResetError):
                    pass  # A short-lived worker may reset its idle keep-alive connection.

            def do_GET(self):
                request_paths.append(self.path)
                self.reply({"data": [{"id": "Qwen/Qwen3.5-0.8B"}]})

            def do_POST(self):
                request_paths.append(self.path)
                json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                self.reply(replies)

            def reply(self, value):
                data = json.dumps(value).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        with HTTPServer(("127.0.0.1", 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                model = nm.NativeModel("Qwen/Qwen3.5-0.8B", "a" * 64,
                                       server.server_port, 2, 10, 128, 16384)
                for _ in range(2):
                    self.assertEqual(model.chat([{"role": "user", "content": "input"}], 0), "answer")
                self.assertEqual(request_paths.count("/v1/chat/completions"), 2)
                self.assertEqual(len(model.calls), 2)
            finally:
                server.shutdown()
                thread.join(5)
                self.assertFalse(thread.is_alive())

    def test_benchmark_shipped_summarizer_arm_and_approval_refusal(self):
        import acon_benchmark as ab
        import generate_observation as gen
        for change_approval in (False, True):
            with tempfile.TemporaryDirectory() as directory:
                root=Path(directory); tasks=root/"tasks"; tasks.mkdir()
                (tasks/"task.json").write_text(json.dumps({"id":"task","family":"fixture","tier":"A",
                    "source":"answer "*20,"query":"What?","gold_answer":"REFERENCE_NOT_EXPORTED","critical_atoms":[]}))
                binary=root/"binary"; binary.write_bytes(b"scripted executable identity")
                approval=root/"approval.json"; approval.write_text(json.dumps({"model_revision":"Qwen/Qwen3.5-0.8B"}))
                runtime=None
                def transport(port,path,body,timeout):
                    return {"data":[{"id":"Qwen/Qwen3.5-0.8B"}]} if path=="models" else self.reply()
                original=nm.NativeModel
                def factory(*args):
                    nonlocal runtime
                    runtime=original(*args,transport=transport); return runtime
                def load(path,model):
                    return (lambda task,seed:model.chat([{"role":"user","content":task["source"]}],seed),
                            {"revision":ab.ACON_REVISION,"source_and_prompt_sha256":{}})
                def worker(args,**kwargs):
                    self.assertNotIn("REFERENCE_NOT_EXPORTED",kwargs["input"])
                    self.assertEqual(args[args.index("--inference-timeout-ms")+1],"25000")
                    self.assertEqual(kwargs["timeout"],35)
                    Path(args[-1]).write_text(json.dumps({"disposition":"generated_unverified","runtime_invoked":True,
                        "inference_usage":{"input_tokens":25,"output_tokens":9},"wall_ms":3}))
                    return subprocess.CompletedProcess(args,0,stdout="summary answer",stderr="")
                run_pilot=ab.run_pilot
                def run(*args,**kwargs):
                    result=run_pilot(*args,**kwargs)
                    if change_approval:approval.write_text(json.dumps({"model_revision":"changed"}))
                    return result
                args=["--run-live","--acon-root",str(root),"--provider","native","--model","Qwen/Qwen3.5-0.8B",
                      "--model-digest","a"*64,"--context-tokens","16384","--max-calls","6","--timeout","25",
                      "--tasks-dir",str(tasks),"--output-dir",str(root/"out"),"--allow-inferred-answers",
                      "--generative-cli-approval",str(approval)]
                with patch.object(nm,"NativeModel",side_effect=factory),patch.object(ab,"load_acon",side_effect=load),\
                     patch.object(gen,"native_guard"),patch.object(gen.subprocess,"run",side_effect=worker),\
                     patch.object(ab,"run_pilot",side_effect=run),patch.object(ab.rp.rb,"_TOKENFOLD_BIN",str(binary)),\
                     patch.object(ab.rp.rb,"TOKENIZER",{"is_exact":True}),patch.object(ab.rp.rb,"count_tokens",len),\
                     patch.object(ab.rp.rb,"compress_tokenfold",side_effect=lambda source,target:source):
                    self.assertEqual(ab.main(args),int(change_approval))
                report=json.loads((root/"out/report.json").read_bytes())
                rows=json.loads((root/"out/economics.json").read_bytes())
                self.assertEqual(len(runtime.calls),6)
                self.assertIn("tokenfold-generative-cli",report["comparisons"])
                self.assertNotIn("tokenfold-generative",report["comparisons"])
                self.assertEqual(report["generative_cli_approval_unchanged"],not change_approval)
                self.assertEqual(report["runtime"]["generative_cli_timeout_ms"],25000)
                self.assertEqual(report["runtime"]["generative_cli_seed_control"],"worker-owned-not-set-by-benchmark")
                cli=next(row for row in rows if row["candidate"]=="tokenfold-generative-cli")
                self.assertEqual(len(cli["model_calls"]),2)
                self.assertEqual(cli["model_calls"][0]["usage"],{"input_tokens":25,"output_tokens":9})
                approval.write_text(json.dumps({"model_revision":"Qwen/Qwen3.5-0.8B"}))
                for flag,value in (("--provider","ollama"),("--timeout","0.0005"),("--max-calls","5")):
                    bad=list(args); bad[bad.index(flag)+1]=value
                    with patch.object(nm,"NativeModel") as initialize,patch.object(gen,"native_guard"):
                        with self.assertRaises(SystemExit):ab.main(bad)
                        initialize.assert_not_called()
                for data in (b"{",b"{}",b"x"*65537):
                    approval.write_bytes(data)
                    with patch.object(nm,"NativeModel") as initialize,patch.object(gen,"native_guard"):
                        with self.assertRaises(SystemExit):ab.main(args)
                        initialize.assert_not_called()

    def test_benchmark_cli_native_provider_and_final_alias_refusal(self):
        import acon_benchmark as ab
        import generate_observation as gen
        for change_alias in (False, True):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                tasks = root / "tasks"
                tasks.mkdir()
                (tasks / "task.json").write_text(json.dumps({"id": "task", "family": "fixture", "tier": "A", "source": "answer " * 20,
                    "query": "What?", "gold_answer": "answer", "critical_atoms": []}))
                binary = root / "binary"
                binary.write_bytes(b"scripted executable identity")
                runtime = None

                def transport(port, path, body, timeout):
                    if path == "models":
                        name = "other" if change_alias and runtime is not None and len(runtime.calls) == 4 else "Qwen/Qwen3.5-0.8B"
                        return {"data": [{"id": name}]}
                    return self.reply()

                original = nm.NativeModel

                def factory(*args):
                    nonlocal runtime
                    runtime = original(*args, transport=transport)
                    return runtime

                def acon_loader(path, model):
                    return (lambda task, seed: model.chat([{"role": "user", "content": task["source"]}], seed),
                            {"revision": ab.ACON_REVISION, "source_and_prompt_sha256": {}})

                args = ["--run-live", "--acon-root", str(root), "--provider", "native",
                        "--native-port", "8001", "--model", "Qwen/Qwen3.5-0.8B",
                        "--model-digest", "a" * 64, "--context-tokens", "16384", "--max-calls", "4", "--timeout", "25",
                        "--tasks-dir", str(tasks), "--output-dir", str(root / "out")]
                with patch.object(nm, "NativeModel", side_effect=factory), patch.object(ab, "load_acon", side_effect=acon_loader), \
                        patch.object(gen, "native_guard") as guard, patch.object(ab.rp.rb, "_TOKENFOLD_BIN", str(binary)), \
                        patch.object(ab.rp.rb, "TOKENIZER", {"is_exact": True}), patch.object(ab.rp.rb, "count_tokens", len), \
                        patch.object(ab.rp.rb, "compress_tokenfold", side_effect=lambda source, target: source):
                    self.assertEqual(ab.main(args), int(change_alias))
                    guard.assert_called()
                report = json.loads((root / "out" / "report.json").read_bytes())
                self.assertEqual(report["provider"], "native")
                self.assertEqual(report["native_loopback_port"], 8001)
                self.assertEqual(report["native_alias_unchanged"], not change_alias)
                self.assertIn("not-weight-attestation", report["model_revision_kind"])
                self.assertEqual(len(runtime.calls), 4)
                self.assertTrue((root / "out" / "economics.json").exists())
                for flag, value in (("--model-digest", "invalid"), ("--native-port", "0"),
                                    ("--timeout", "120"), ("--context-tokens", "20000"),
                                    ("--provider", "ollama")):
                    invalid = list(args)
                    invalid[invalid.index(flag) + 1] = value
                    with patch.object(nm, "NativeModel") as initialize, patch.object(gen, "native_guard"):
                        with self.assertRaises(SystemExit):
                            ab.main(invalid)
                        initialize.assert_not_called()


if __name__ == "__main__":
    unittest.main()
