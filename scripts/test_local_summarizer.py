import importlib.util
import json
import os
import hashlib
import subprocess
import sys
from pathlib import Path
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch, MagicMock
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("bridge", Path(__file__).with_name("local-summarizer.py"))
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)


class BridgeTest(unittest.TestCase):
    def test_opt_in_short_ids_bind_to_original_optional_groups_and_fail_closed(self):
        request={"model_revision":"Qwen/Qwen3.5-4B","instruction":"Summarize","query":"A?","target_tokens":100,
                 "source":{"prefix":"P","suffix":"S","groups":[
                     {"id":"s2","text":"Fixed","required":True},
                     {"id":"s1","text":"A = 007"},{"id":"s0","text":"B = 008"}]}}
        before=json.dumps(request)
        payload=bridge.build_payload(request,128,True)
        prompt=json.loads(payload["messages"][1]["content"])
        self.assertEqual(prompt["protected_context"],"PFixedS")
        self.assertEqual(prompt["groups"],[{"id":"s0","text":"A = 007"},{"id":"s1","text":"B = 008"}])
        self.assertEqual(payload["response_format"]["json_schema"]["schema"]["properties"]["source_ids"]["items"]["enum"],["s0","s1"])
        self.assertEqual(json.dumps(request),before)
        value={"schema_version":1,"model_revision":request["model_revision"],"summary":"A = 007","source_ids":["s1","s0","s0"]}
        result={"model":request["model_revision"],"choices":[{"finish_reason":"stop","message":{"content":json.dumps(value)}}],
                "usage":{"prompt_tokens":17,"completion_tokens":8}}
        self.assertEqual(bridge.candidate_from_result(request,result,True)["source_ids"],["s0","s1","s1"])
        self.assertEqual(bridge.candidate_from_result(request,result)["source_ids"],value["source_ids"])
        for bad in (["s2"],["unknown"],[1],{"s0":True},"s0"):
            value["source_ids"]=bad;result["choices"][0]["message"]["content"]=json.dumps(value)
            candidate=bridge.candidate_from_result(request,result,True)
            self.assertEqual(candidate["generation_failure"],"malformed_candidate")
            self.assertEqual(candidate["summary"],"")
            self.assertEqual(candidate["source_ids"],[])
            self.assertEqual(candidate["inference_usage"],{"input_tokens":17,"output_tokens":8})
        result["choices"][0]["finish_reason"]="length"
        self.assertEqual(bridge.candidate_from_result(request,result,True)["generation_failure"],"incomplete_response")
        request["source"]["groups"]=[{"id":str(i)+"x"*240,"text":"G"} for i in range(300)]
        value["source_ids"]=["s"+str(i) for i in range(300)]
        result["choices"][0]={"finish_reason":"stop","message":{"content":json.dumps(value)}}
        candidate=bridge.candidate_from_result(request,result,True)
        self.assertEqual(candidate["generation_failure"],"malformed_candidate")
        self.assertEqual(candidate["summary"],"")
        self.assertEqual(candidate["inference_usage"],{"input_tokens":17,"output_tokens":8})

    def test_qwen_schema_selects_evidence_before_summary(self):
        payload=bridge.build_payload({"model_revision":"Qwen/Qwen3.5-0.8B",
            "instruction":"Summarize","query":"A?","target_tokens":100,
            "source":{"groups":[{"id":"fixed","text":"Protected","required":True},
                                {"id":"a","text":"A = 007"}]}},128)
        schema=payload["response_format"]["json_schema"]["schema"]
        self.assertEqual(list(schema["properties"]),
                         ["schema_version","model_revision","source_ids","summary"])
        self.assertEqual(schema["properties"]["source_ids"]["items"]["enum"],["a"])
        self.assertEqual(schema["properties"]["source_ids"]["maxItems"],1)
        self.assertEqual(schema["properties"]["source_ids"]["minItems"],1)
        self.assertEqual(schema["required"],list(schema["properties"]))
        self.assertEqual(payload["chat_template_kwargs"],{"enable_thinking":False})

    @unittest.skipUnless(os.name == "nt", "Windows cleared-environment regression")
    def test_windows_identity_is_reconstructed_without_inheriting_environment(self):
        with tempfile.TemporaryDirectory() as root:
            request=Path(root)/"request.json"
            request.write_text(json.dumps({"kind":"generative_summary","schema_version":1,
                "model_revision":"pinned","instruction":"Summarize","query":"A?",
                "target_tokens":100,"source":{"groups":[{"id":"a","text":"A = 007"}]}}))
            script=Path(__file__).with_name("local-summarizer.py").resolve()
            code="import importlib.util,os,getpass,json; s=importlib.util.spec_from_file_location('bridge',"+repr(str(script))+"); b=importlib.util.module_from_spec(s); s.loader.exec_module(b); b.prepare_request(100); print(json.dumps({'root':bool(os.environ.get('SystemRoot')),'user':bool(os.environ.get('USERNAME')) and getpass.getuser()==os.environ['USERNAME']}))"
            result=subprocess.run([sys.executable,"-c",code],env={"TOKENFOLD_SELECT_REQUEST_PATH":str(request)},capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0)
            self.assertEqual(json.loads(result.stdout),{"root":True,"user":True})

    def test_direct_loader_is_offline_bounded_and_preserves_usage_on_rejection(self):
        with tempfile.TemporaryDirectory() as root:
            request, response = Path(root)/"request.json", Path(root)/"response.json"
            request.write_text(json.dumps({"kind":"generative_summary","schema_version":1,
                "model_revision":"Qwen/Qwen3.5-0.8B","instruction":"Summarize","query":"A?",
                "target_tokens":100,"source":{"groups":[{"id":"a","text":"A = 007"}]}}))
            torch=MagicMock()
            torch.cuda.is_available.return_value=False
            tokenizer=MagicMock()
            inputs=MagicMock()
            inputs.to.return_value=inputs
            inputs.keys.return_value=["input_ids"]
            inputs.__getitem__.return_value.shape=(1,12)
            tokenizer.apply_chat_template.return_value=inputs
            tokenizer.decode.return_value=json.dumps({"schema_version":1,"model_revision":"Qwen/Qwen3.5-0.8B",
                "summary":"A has code 007","source_ids":["a"]})
            model=MagicMock()
            model.to.return_value=model
            model.eval.return_value=model
            model.generate.return_value=[[0]*12+[4,5,2]]
            model.generation_config.eos_token_id=[2]
            loader=MagicMock()
            loader.from_pretrained.return_value=(model,{"missing_keys":[],"mismatched_keys":[],"error_msgs":[]})
            auto=MagicMock()
            auto.from_pretrained.return_value=tokenizer
            hf=SimpleNamespace(AutoTokenizer=auto,Qwen3_5ForCausalLM=loader)
            with patch.dict(sys.modules,{"torch":torch,"transformers":hf}), patch.dict(os.environ,
                {"TOKENFOLD_SELECT_REQUEST_PATH":str(request),"TOKENFOLD_SELECT_RESPONSE_PATH":str(response)}):
                with self.assertRaises(ValueError):
                    bridge.run_transformers(root,1024,"cuda")
                loader.from_pretrained.assert_not_called()
                bridge.run_transformers(root,1024)
                candidate=json.loads(response.read_bytes())
                self.assertEqual(candidate["inference_usage"],{"input_tokens":12,"output_tokens":3})
                self.assertEqual(candidate["source_ids"],["a"])
                self.assertEqual(os.environ["HF_HUB_OFFLINE"],"1")
                self.assertEqual(os.environ["TRANSFORMERS_OFFLINE"],"1")
                kwargs=loader.from_pretrained.call_args.kwargs
                self.assertTrue(kwargs["local_files_only"] and kwargs["use_safetensors"])
                self.assertFalse(kwargs["trust_remote_code"])
                self.assertEqual(kwargs["attn_implementation"],"sdpa")
                self.assertFalse(tokenizer.apply_chat_template.call_args.kwargs["enable_thinking"])
                self.assertTrue(model.generate.call_args.kwargs["use_cache"])
                self.assertFalse(model.generate.call_args.kwargs["do_sample"])
                # Resident mode loads once and performs distinct requests against
                # the same model/tokenizer, not a generated-text cache.
                loader.from_pretrained.reset_mock()
                model.generate.reset_mock()
                server=bridge.resident_server(root,"0.8b",0,1024,"cpu")
                self.assertEqual(server.RequestHandlerClass.protocol_version,"HTTP/1.1")
                self.assertEqual(server.RequestHandlerClass.timeout,5)
                thread=threading.Thread(target=server.serve_forever)
                thread.start()
                try:
                    for query in ("A?","A now?"):
                        value=json.loads(request.read_bytes());value["query"]=query
                        request.write_text(json.dumps(value))
                        bridge.run_resident(server.server_port,1024)
                        candidate=json.loads(response.read_bytes())
                        self.assertEqual(candidate["source_ids"],["a"],candidate.get("generation_failure"))
                    self.assertEqual(loader.from_pretrained.call_count,1)
                    self.assertEqual(model.generate.call_count,2)
                    bridge.run_resident(server.server_port,8192)
                    self.assertEqual(json.loads(response.read_bytes())["generation_failure"],"http_client_error")
                    self.assertEqual(model.generate.call_count,2)
                finally:
                    server.shutdown();thread.join();server.server_close()
                model.generate.return_value=[[0]*12+[4,5]]  # Output cap without EOS.
                bridge.run_transformers(root,1024)
                candidate=json.loads(response.read_bytes())
                self.assertEqual(candidate["generation_failure"],"incomplete_response")
                self.assertEqual(candidate["inference_usage"]["output_tokens"],2)
                model.generate.reset_mock()
                loader.from_pretrained.reset_mock()
                inputs.__getitem__.return_value.shape=(1,16384)
                with self.assertRaises(ValueError):
                    bridge.run_transformers(root,1024)
                model.generate.assert_not_called()
                loader.from_pretrained.assert_not_called()
                inputs.__getitem__.return_value.shape=(1,12)
                for key in ("missing_keys","mismatched_keys","error_msgs"):
                    loader.from_pretrained.return_value=(model,{key:["incomplete weights"]})
                    with self.assertRaises(ValueError):
                        bridge.run_transformers(root,1024)
                model.generate.assert_not_called()

    def test_fixed_transport_failures_never_export_exception_text(self):
        with tempfile.TemporaryDirectory() as root:
            request, response = Path(root)/"request.json", Path(root)/"response.json"
            request.write_text(json.dumps({"kind":"generative_summary","schema_version":1,
                "model_revision":"pinned","instruction":"Summarize","query":"A?",
                "target_tokens":100,"source":{"groups":[{"id":"a","text":"A = 007"}]}}))
            with patch.dict(os.environ,{"TOKENFOLD_SELECT_REQUEST_PATH":str(request),
                                       "TOKENFOLD_SELECT_RESPONSE_PATH":str(response)}):
                for error, reason in ((TimeoutError("private"),"runtime_timeout"),
                                      (OSError("private"),"transport_failed")):
                    with patch("http.client.HTTPConnection") as connection:
                        connection.return_value.request.side_effect=error
                        bridge.run(8080,1024)
                        connection.return_value.request.assert_called_once()
                        connection.return_value.close.assert_called_once()
                    candidate=json.loads(response.read_bytes())
                    self.assertEqual(candidate["generation_failure"],reason)
                    self.assertNotIn(b"private",response.read_bytes())
                    self.assertNotIn("inference_usage",candidate)
                for body in (b"[]",b"not JSON",b"x"*(1024*1024+1)):
                    with patch("http.client.HTTPConnection") as connection:
                        reply=connection.return_value.getresponse.return_value
                        reply.status=200
                        reply.read.return_value=body
                        bridge.run(8080,1024)
                    self.assertEqual(json.loads(response.read_bytes())["generation_failure"],"invalid_runtime_response")

    def test_qwen_profiles_are_explicit_and_default_is_small(self):
        with tempfile.TemporaryDirectory() as root:
            for size in ("0.8b", "2b", "4b"):
                self.assertEqual(bridge.approval_config(size,"transformers",8000,root,"test",root)["model_revision"],"Qwen/Qwen3.5-"+size.upper())
                self.assertEqual(bridge.approval_config(size,"openai",8000,root,"test")["model_revision"],"Qwen/Qwen3.5-"+size.upper())
            output=Path(root)/"approval.json"
            args=[sys.executable,str(Path(__file__).with_name("local-summarizer.py")),"--write-config",str(output),"--scratch-root",root,"--approved-by","test","--model-dir",root]
            self.assertEqual(subprocess.run(args,capture_output=True).returncode,0)
            self.assertEqual(json.loads(output.read_text())["model_revision"],"Qwen/Qwen3.5-0.8B")
            self.assertIn("resident",json.loads(output.read_text())["arguments"])
            self.assertIn("256",bridge.approval_config("0.8b","transformers",8000,root,"test",root,256)["arguments"])
            self.assertIn("cuda",bridge.approval_config("0.8b","transformers",8000,root,"test",root,256,"cuda")["arguments"])
            with self.assertRaises(ValueError):
                bridge.approval_config("0.8b","transformers",8000,root,"test")
            with self.assertRaises(ValueError):
                bridge.approval_config("0.8b","ollama",11434,root,"test")
            self.assertIn("--short-source-ids",bridge.approval_config("4b","openai",8000,root,"test",short_source_ids=True)["arguments"])
            self.assertNotIn("--short-source-ids",bridge.approval_config("4b","openai",8000,root,"test")["arguments"])
            with self.assertRaises(ValueError):
                bridge.approval_config("4b","resident",8000,root,"test",short_source_ids=True)
            self.assertNotEqual(subprocess.run(args,capture_output=True).returncode,0)

    def test_real_loopback_call_and_fail_closed_model_completion(self):
        seen = []
        status = 200
        reply = {"model": "pinned", "usage":{"prompt_tokens":123,"completion_tokens":45}, "choices": [{"finish_reason": "stop", "message": {
            "content": json.dumps({"schema_version": 1, "model_revision": "pinned",
                                   "summary": "A has code 007", "source_ids": ["a"]})}}]}

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            def handle(self):
                self.response_sent = False
                try:
                    super().handle()
                except ConnectionResetError:
                    # Windows can reset an idle keep-alive after the client consumed
                    # the response. Never suppress an error before sending it.
                    if not self.response_sent:
                        raise

            def do_POST(self):
                seen.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
                body=json.dumps(reply).encode()
                self.send_response(status)
                self.send_header("Content-Length",str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                self.wfile.flush()
                self.response_sent = True

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as root:
                request, response = Path(root)/"request.json", Path(root)/"response.json"
                request.write_text(json.dumps({"kind": "generative_summary", "schema_version": 1,
                    "model_revision": "pinned", "instruction": "Summarize", "query": "A?",
                    "target_tokens": 100, "source": {"groups": [{"id": "a", "text": "A = 007"}]}}))
                with patch.dict(os.environ, {"TOKENFOLD_SELECT_REQUEST_PATH": str(request),
                    "TOKENFOLD_SELECT_RESPONSE_PATH": str(response), "HTTP_PROXY": "http://invalid:1"}):
                    bridge.run(server.server_port, 1024)
                    self.assertEqual(json.loads(response.read_bytes())["source_ids"], ["a"])
                    self.assertEqual(seen[0]["model"], "pinned")
                    # End-to-end native CLI -> env-cleared Python -> loopback server.
                    binary = Path(__file__).resolve().parents[1] / "target" / "debug" / ("tokenfold.exe" if os.name == "nt" else "tokenfold")
                    source, approval = Path(root)/"source.json", Path(root)/"approval.json"
                    source.write_text(json.dumps({"prefix":"P","suffix":"S","groups":[
                        {"id":"fixed","text":"KEEP","required":True},
                        {"id":"a","text":"A = 007"},{"id":"b","text":"filler "*1000}]}))
                    approval.write_text(json.dumps({"executable":sys.executable,
                        "executable_sha256":hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest(),
                        "arguments":[str(Path(__file__).with_name("local-summarizer.py").resolve()),"--port",str(server.server_port)],
                        "model_revision":"pinned","approved_by":"synthetic regression","scratch_root":root}))
                    artifact=Path(root)/"summary-artifact.json"
                    result = subprocess.run([str(binary),"summarize",str(source),"--experimental",
                        "--query","A?","--target-tokens","512","--model-config",str(approval),
                        "--inference-timeout-ms","30000","--save-summary",str(artifact)],capture_output=True,timeout=35)
                    self.assertEqual(result.returncode,0)
                    receipt=json.loads(result.stderr)
                    self.assertEqual(receipt["disposition"],"generated_unverified")
                    self.assertTrue(receipt["budget_met"])
                    generator_prompt=json.loads(seen[1]["messages"][1]["content"])
                    self.assertEqual([g["id"] for g in generator_prompt["groups"]],["a","b"])
                    self.assertEqual(generator_prompt["protected_context"],"PKEEPS")
                    self.assertEqual(receipt["inference_usage"], {"input_tokens":123,"output_tokens":45})
                    self.assertEqual(receipt["usage_provenance"],"approved-runtime-reported")
                    self.assertTrue(result.stdout.startswith(b"PKEEP{") and result.stdout.endswith(b"}S"))
                    self.assertIn(b"A = 007",result.stdout)
                    original_artifact=artifact.read_bytes()
                    collision=subprocess.run([str(binary),"summarize",str(source),"--experimental",
                        "--query","A?","--target-tokens","512","--model-config",str(approval),
                        "--reuse-summary",str(artifact),"--output",str(artifact)],capture_output=True,timeout=10)
                    self.assertNotEqual(collision.returncode,0)
                    self.assertEqual(artifact.read_bytes(),original_artifact)
                    unavailable=subprocess.run([str(binary),"summarize",str(source),"--experimental",
                        "--query","A?","--target-tokens","512","--model-config",str(approval),
                        "--reuse-summary",str(Path(root)/"missing-directory"/"artifact.json")],capture_output=True,timeout=10)
                    self.assertEqual(unavailable.returncode,0)
                    self.assertEqual(json.loads(unavailable.stderr)["fallback_reason"],"artifact_unavailable")
                    generation_count=len(seen)
                    def reuse(query="A?"):
                        return subprocess.run([str(binary),"summarize",str(source),"--experimental",
                            "--query",query,"--target-tokens","512","--model-config",str(approval),
                            "--reuse-summary",str(artifact)],capture_output=True,timeout=10)
                    reused=reuse()
                    self.assertEqual(reused.returncode,0)
                    self.assertEqual(reused.stdout,result.stdout)
                    reused_receipt=json.loads(reused.stderr)
                    self.assertTrue(reused_receipt["artifact_reused"])
                    self.assertFalse(reused_receipt["runtime_invoked"])
                    self.assertIsNone(reused_receipt["inference_usage"])
                    self.assertEqual(reused_receipt["reused_inference_usage"],{"input_tokens":123,"output_tokens":45})
                    changed=reuse("Different question?")
                    self.assertEqual(json.loads(changed.stderr)["fallback_reason"],"artifact_context_mismatch")
                    poisoned=json.loads(original_artifact)
                    poisoned["candidate"]["summary"]="sk-"+"x"*25
                    artifact.write_text(json.dumps(poisoned))
                    rejected=reuse()
                    self.assertEqual(json.loads(rejected.stderr)["fallback_reason"],"output_guard_failed")
                    self.assertFalse(json.loads(rejected.stderr)["artifact_reused"])
                    self.assertNotIn(("sk-"+"x"*25).encode(),rejected.stdout+rejected.stderr)
                    self.assertEqual(len(seen),generation_count)
                    artifact.write_bytes(original_artifact)
                    fallback = subprocess.run([str(binary),"summarize",str(source),"--experimental",
                        "--query","A?","--target-tokens","16","--model-config",str(approval),
                        "--inference-timeout-ms","30000"],capture_output=True,timeout=35)
                    self.assertEqual(fallback.returncode,0)
                    fallback_receipt=json.loads(fallback.stderr)
                    self.assertEqual(fallback_receipt["fallback_reason"],"over_budget")
                    self.assertEqual(fallback_receipt["inference_usage"],{"input_tokens":123,"output_tokens":45})
                    original=json.loads(source.read_bytes())
                    self.assertEqual(fallback.stdout,(original["prefix"]+"".join(g["text"] for g in original["groups"])+original["suffix"]).encode())
                    response.unlink()
                    reply["model"] = "other"
                    bridge.run(server.server_port, 1024)
                    failed=json.loads(response.read_bytes())
                    self.assertEqual(failed["generation_failure"],"model_mismatch")
                    self.assertEqual(failed["inference_usage"],{"input_tokens":123,"output_tokens":45})
                    self.assertEqual(failed["summary"],"")
                    response.unlink()
                    reply["model"] = "pinned"
                    reply["choices"][0]["finish_reason"] = "length"
                    bridge.run(server.server_port, 1024)
                    failed=json.loads(response.read_bytes())
                    self.assertEqual(failed["generation_failure"],"incomplete_response")
                    self.assertEqual(failed["inference_usage"],{"input_tokens":123,"output_tokens":45})
                    # A real native caller must retain usage without admitting this failure.
                    incomplete=subprocess.run([str(binary),"summarize",str(source),"--experimental",
                        "--query","A?","--target-tokens","512","--model-config",str(approval),
                        "--inference-timeout-ms","30000"],capture_output=True,timeout=35)
                    self.assertEqual(incomplete.returncode,0)
                    incomplete_receipt=json.loads(incomplete.stderr)
                    self.assertEqual(incomplete_receipt["fallback_reason"],"incomplete_response")
                    self.assertEqual(incomplete_receipt["inference_usage"],{"input_tokens":123,"output_tokens":45})
                    self.assertEqual(incomplete.stdout,fallback.stdout)
                    response.unlink()
                    reply["choices"][0]["finish_reason"] = "stop"
                    reply["usage"]["prompt_tokens"] = True
                    bridge.run(server.server_port, 1024)
                    self.assertNotIn("inference_usage",json.loads(response.read_bytes()))
                    response.unlink()
                    reply["choices"][0]["message"]["content"] = '{"schema_version":1,"schema_version":1}'
                    bridge.run(server.server_port, 1024)
                    failed=json.loads(response.read_bytes())
                    self.assertEqual(failed["generation_failure"],"malformed_candidate")
                    self.assertEqual(failed["summary"],"")
                    # Server diagnostics may contain private data. Only fixed HTTP
                    # classifications cross the native receipt; no retries occur.
                    reply.clear()
                    reply["error"] = "sk-"+"x"*25
                    for code, reason in ((400,"http_client_error"),(500,"http_server_error"),(302,"http_unexpected_status")):
                        status = code
                        count = len(seen)
                        failed_call=subprocess.run([str(binary),"summarize",str(source),"--experimental",
                            "--query","A?","--target-tokens","512","--model-config",str(approval),
                            "--inference-timeout-ms","30000"],capture_output=True,timeout=35)
                        self.assertEqual(failed_call.returncode,0)
                        failed_receipt=json.loads(failed_call.stderr)
                        self.assertEqual(failed_receipt["fallback_reason"],reason)
                        self.assertIsNone(failed_receipt["inference_usage"])
                        self.assertEqual(failed_call.stdout,fallback.stdout)
                        self.assertNotIn(reply["error"].encode(),failed_call.stdout+failed_call.stderr)
                        self.assertEqual(len(seen),count+1)
        finally:
            server.shutdown()
            thread.join()
            server.server_close()


if __name__ == "__main__":
    unittest.main()
