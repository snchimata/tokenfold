"""Approved local Qwen summarizer: direct Transformers or a loopback model server.
No implicit downloads, credentials, redirects, proxies, retries or descendants.
"""
import argparse
import json
import os
from pathlib import Path


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON field")
        result[key] = value
    return result


def write_failure(request, reason):
    # Fixed classifications only: never persist server bodies or exception messages.
    candidate = {"schema_version":1, "model_revision":request["model_revision"],
                 "summary":"", "source_ids":[], "generation_failure":reason}
    Path(os.environ["TOKENFOLD_SELECT_RESPONSE_PATH"]).write_bytes(json.dumps(candidate).encode())


def prepare_request(output_tokens, short_source_ids=False):
    if os.name == "nt":
        # Reconstruct OS facts, never inherit caller credentials/proxies. GPU torch
        # imports getpass, which otherwise tries Unix-only pwd in an empty environment.
        import ctypes
        buffer = ctypes.create_unicode_buffer(32768)
        if "SystemRoot" not in os.environ:
            size = ctypes.windll.kernel32.GetWindowsDirectoryW(buffer, len(buffer))
            if not 0 < size < len(buffer):
                raise ValueError("Windows directory unavailable")
            os.environ["SystemRoot"] = buffer.value
        if "USERNAME" not in os.environ:
            size = ctypes.c_ulong(len(buffer))
            if not ctypes.windll.advapi32.GetUserNameW(buffer, ctypes.byref(size)):
                raise ValueError("Windows user unavailable")
            os.environ["USERNAME"] = buffer.value
    request = json.loads(Path(os.environ["TOKENFOLD_SELECT_REQUEST_PATH"]).read_bytes())
    if request["kind"] != "generative_summary" or request["schema_version"] != 1:
        raise ValueError("unsupported request")
    return request, build_payload(request, output_tokens, short_source_ids)


def build_payload(request, output_tokens, short_source_ids=False):
    source = request["source"]
    prompt = {"model_revision":request["model_revision"], "query":request["query"],
              "target_tokens":request["target_tokens"],
              "protected_context":source.get("prefix", "") + "".join(g["text"] for g in source["groups"] if g.get("required",False)) + source.get("suffix", ""),
              "groups":[{"id":"s"+str(i) if short_source_ids else g["id"],"text":g["text"]}
                        for i,g in enumerate(g for g in source["groups"] if not g.get("required",False))]}
    payload = {"model": request["model_revision"], "stream": False,
               "max_tokens": output_tokens, "temperature": 0,
               "messages": [{"role": "system", "content": request["instruction"]},
                            {"role": "user", "content": json.dumps(prompt, ensure_ascii=False)}]}
    if request["model_revision"].lower().startswith(("qwen3.5:", "qwen/qwen3.5-")):
        # Bounded compression uses non-thinking mode; no retry/escalation if unsupported.
        if request["model_revision"].startswith("qwen3.5:"):
            payload["reasoning_effort"] = "none"
        else:
            payload["chat_template_kwargs"] = {"enable_thinking":False}
        ids = [g["id"] for g in prompt["groups"]]
        payload["response_format"] = {"type":"json_schema","json_schema":{"name":"tokenfold_summary","strict":True,
            "schema":{"type":"object","additionalProperties":False,
                "required":["schema_version","model_revision","source_ids","summary"],"properties":{
                    "schema_version":{"type":"integer","const":1},
                    "model_revision":{"type":"string","const":request["model_revision"]},
                    "source_ids":{"type":"array","minItems":1,"maxItems":len(ids),"items":{"type":"string","enum":ids}},"summary":{"type":"string"}}}}}
    return payload


def transformer_components(model_dir, device):
    if device not in {"auto", "cpu", "cuda"}:
        raise ValueError("invalid device")
    model_dir = Path(model_dir).resolve(strict=True)
    if not model_dir.is_dir():
        raise ValueError("local model directory required")
    # Offline, safe-tensor-only loading: source data never triggers a Hub request
    # or downloaded Python code. Install dependencies/download weights separately.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
    os.environ["TOKENIZERS_PARALLELISM"] = "false"
    import torch
    from transformers import AutoTokenizer, Qwen3_5ForCausalLM

    available = torch.cuda.is_available()
    if device == "cuda" and not available:
        raise ValueError("requested CUDA runtime unavailable")
    device = ("cuda" if available else "cpu") if device == "auto" else device
    dtype = (torch.bfloat16 if torch.cuda.is_bf16_supported() else torch.float16) if device == "cuda" else torch.float32
    tokenizer = AutoTokenizer.from_pretrained(str(model_dir), local_files_only=True, trust_remote_code=False)
    return torch, tokenizer, Qwen3_5ForCausalLM, model_dir, device, dtype


def encode_input(tokenizer, payload, device, output_tokens):
    inputs = tokenizer.apply_chat_template(payload["messages"], tokenize=True,
        add_generation_prompt=True, enable_thinking=False, return_dict=True,
        return_tensors="pt").to(device)
    input_tokens = inputs["input_ids"].shape[-1]
    if input_tokens + output_tokens > 16384:
        raise ValueError("local context allowance exceeded")  # Never truncate source.
    return inputs


def load_text_model(loader, model_dir, device, dtype):
    # The text-only class maps the official multimodal checkpoint's language keys;
    # never accept missing/randomly initialized language weights after conversion.
    model, loading = loader.from_pretrained(str(model_dir),
        local_files_only=True, trust_remote_code=False, use_safetensors=True,
        dtype=dtype, attn_implementation="sdpa", output_loading_info=True)
    if loading.get("missing_keys") or loading.get("mismatched_keys") or loading.get("error_msgs"):
        raise ValueError("incomplete text model weights")
    return model.to(device).eval()


def generate_result(torch, tokenizer, model, inputs, output_tokens, revision):
    input_tokens = inputs["input_ids"].shape[-1]
    with torch.inference_mode():
        outputs = model.generate(**inputs, max_new_tokens=output_tokens,
                                 do_sample=False, use_cache=True)
    generated = outputs[0][input_tokens:]
    eos = model.generation_config.eos_token_id
    eos = [eos] if isinstance(eos, int) else eos or []
    complete = len(generated) > 0 and int(generated[-1]) in eos
    return {"model":revision,
              "usage":{"prompt_tokens":input_tokens,"completion_tokens":len(generated)},
              "choices":[{"finish_reason":"stop" if complete else "length",
                          "message":{"content":tokenizer.decode(generated, skip_special_tokens=True)}}]}


def run_transformers(model_dir, output_tokens, device="auto"):
    request, payload = prepare_request(output_tokens)
    torch, tokenizer, loader, model_dir, device, dtype = transformer_components(model_dir, device)
    inputs = encode_input(tokenizer, payload, device, output_tokens)
    model = load_text_model(loader, model_dir, device, dtype)
    write_result(request, generate_result(torch, tokenizer, model, inputs, output_tokens, request["model_revision"]))


def run(port, output_tokens, short_source_ids=False):
    request, payload = prepare_request(output_tokens, short_source_ids)
    import http.client
    # HTTPConnection has no proxy discovery or redirect support; address is fixed loopback.
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=25)
    try:
        connection.request("POST", "/v1/chat/completions", body=json.dumps(payload).encode(),
                           headers={"Content-Type": "application/json"})
        response = connection.getresponse()
        if response.status != 200:
            reason = "http_client_error" if 400 <= response.status < 500 else "http_server_error" if 500 <= response.status < 600 else "http_unexpected_status"
            write_failure(request, reason)
            return
        body = response.read(1024 * 1024 + 1)
    except TimeoutError:
        write_failure(request, "runtime_timeout")
        return
    except (OSError, http.client.HTTPException):
        write_failure(request, "transport_failed")
        return
    finally:
        connection.close()
    if len(body) > 1024 * 1024:
        write_failure(request, "invalid_runtime_response")
        return
    try:
        result = json.loads(body, object_pairs_hook=unique_object)
        if not isinstance(result, dict):
            raise ValueError("invalid runtime envelope")
    except (ValueError, UnicodeError):
        write_failure(request, "invalid_runtime_response")
        return
    write_result(request, result, short_source_ids)


def write_result(request, result, short_source_ids=False):
    candidate = candidate_from_result(request, result, short_source_ids)
    Path(os.environ["TOKENFOLD_SELECT_RESPONSE_PATH"]).write_bytes(json.dumps(candidate, ensure_ascii=False).encode())


def candidate_from_result(request, result, short_source_ids=False):
    usage = result.get("usage")
    counters = None
    if isinstance(usage, dict) and all(type(usage.get(k)) is int and 0 <= usage[k] < 2**64
                                      for k in ("prompt_tokens", "completion_tokens")):
        counters = {"input_tokens": usage["prompt_tokens"], "output_tokens": usage["completion_tokens"]}
    failure = None
    if result.get("model") != request["model_revision"]:
        failure = "model_mismatch"
    else:
        choices = result.get("choices")
        if not isinstance(choices, list) or len(choices) != 1 or not isinstance(choices[0], dict) or choices[0].get("finish_reason") != "stop":
            failure = "incomplete_response"
        else:
            try:
                text = choices[0]["message"]["content"]
                if not isinstance(text, str) or len(text.encode()) > 65536:
                    raise ValueError("invalid candidate")
                candidate = json.loads(text, object_pairs_hook=unique_object)
                if not isinstance(candidate, dict) or set(candidate) != {"schema_version", "model_revision", "summary", "source_ids"}:
                    raise ValueError("invalid candidate fields")
                if short_source_ids:
                    aliases = {"s"+str(i):g["id"] for i,g in enumerate(
                        g for g in request["source"]["groups"] if not g.get("required",False))}
                    if not isinstance(candidate["source_ids"], list):
                        raise ValueError("invalid candidate IDs")
                    candidate["source_ids"] = [aliases[identity] for identity in candidate["source_ids"]]
            except (ValueError, KeyError, TypeError):
                failure = "malformed_candidate"
    if failure is not None:
        # No rejected reply or server diagnostics survive; counters still represent consumed work.
        candidate = {"schema_version":1, "model_revision":request["model_revision"],
                     "summary":"", "source_ids":[], "generation_failure":failure}
    if counters is not None:
        candidate["inference_usage"] = counters
    encoded = json.dumps(candidate, ensure_ascii=False).encode()
    if len(encoded) > 65536:
        # Alias expansion can exceed the candidate limit; discard text, not known consumption.
        candidate = {"schema_version":1, "model_revision":request["model_revision"],
                     "summary":"", "source_ids":[], "generation_failure":"malformed_candidate"}
        if counters is not None:
            candidate["inference_usage"] = counters
    return candidate


def resident_server(model_dir, size, port, output_tokens, device):
    from http.server import BaseHTTPRequestHandler, HTTPServer
    torch, tokenizer, loader, model_dir, device, dtype = transformer_components(model_dir, device)
    model = load_text_model(loader, model_dir, device, dtype)
    revision = "Qwen/Qwen3.5-" + size.upper()

    class Handler(BaseHTTPRequestHandler):
        # Let clients consume length-delimited replies before closing the connection.
        protocol_version = "HTTP/1.1"
        timeout = 5  # Stdlib setup bounds headers, bodies and idle keep-alive reads.

        def log_message(self, *args):
            pass  # Never log source, requests or rejected model text.

        def reply(self, status, value):
            body = json.dumps(value, ensure_ascii=False).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path != "/health":
                self.reply(404, {})
                return
            self.reply(200, {"ready":True,"model_revision":revision,"model_loads":1})

        def do_POST(self):
            try:
                if self.path != "/tokenfold/summarize" or self.headers.get("Transfer-Encoding"):
                    self.reply(400, {})
                    return
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 2 * 1024 * 1024:
                    self.reply(400, {})
                    return
                request = json.loads(self.rfile.read(length), object_pairs_hook=unique_object)
                if request.get("kind") != "generative_summary" or request.get("schema_version") != 1 or request.get("model_revision") != revision:
                    self.reply(400, {})
                    return
                allowance = request.get("max_output_tokens")
                if type(allowance) is not int or not 1 <= allowance <= output_tokens:
                    self.reply(400, {})
                    return
                payload = build_payload(request, allowance)
                inputs = encode_input(tokenizer, payload, device, allowance)
                result = generate_result(torch, tokenizer, model, inputs, allowance, revision)
                self.reply(200, candidate_from_result(request, result))
            except (BrokenPipeError, ConnectionResetError):
                pass
            except Exception:
                self.reply(500, {})  # No exception/server body/source export.

    # ponytail: one serialized request, one resident model, no batching/daemon manager.
    # Explicit caller-owned server; native child deadlines cannot kill server kernels.
    return HTTPServer(("127.0.0.1", port), Handler)


def run_resident(port, output_tokens):
    request, _ = prepare_request(1)
    request["max_output_tokens"] = output_tokens
    import http.client
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=25)
    try:
        connection.request("POST", "/tokenfold/summarize", body=json.dumps(request).encode(), headers={"Content-Type":"application/json"})
        response = connection.getresponse()
        if response.status != 200:
            write_failure(request, "http_client_error" if 400 <= response.status < 500 else "http_server_error" if 500 <= response.status < 600 else "http_unexpected_status")
            return
        body = response.read(65537)
        if len(body) > 65536:
            write_failure(request, "invalid_runtime_response")
            return
        # Native code revalidates the candidate, evidence, secrets and full budget.
        Path(os.environ["TOKENFOLD_SELECT_RESPONSE_PATH"]).write_bytes(body)
    except TimeoutError:
        write_failure(request, "runtime_timeout")
    except (OSError, http.client.HTTPException):
        write_failure(request, "transport_failed")
    finally:
        connection.close()


def approval_config(size, backend, port, scratch_root, approved_by, model_dir=None, output_tokens=1024, device="auto", short_source_ids=False):
    import hashlib
    import sys
    if size not in {"0.8b", "2b", "4b"} or backend not in {"transformers", "resident", "openai"}:
        raise ValueError("unsupported summarizer profile")
    if type(output_tokens) is not int or not 1 <= output_tokens <= 8192:
        raise ValueError("invalid output allowance")
    if device not in {"auto", "cpu", "cuda"}:
        raise ValueError("invalid device")
    if short_source_ids and backend != "openai":
        raise ValueError("short source IDs require the loopback openai backend")
    scratch = Path(scratch_root).resolve(strict=True)
    if not scratch.is_dir() or not approved_by.strip():
        raise ValueError("private scratch directory and approver required")
    model = "Qwen/Qwen3.5-" + size.upper()
    arguments = [str(Path(__file__).resolve()), "--backend", backend, "--max-output-tokens", str(output_tokens)]
    if backend == "transformers":
        if model_dir is None or not Path(model_dir).resolve(strict=True).is_dir():
            raise ValueError("--model-dir must be an existing local snapshot")
        arguments += ["--model-dir", str(Path(model_dir).resolve()), "--device", device]
    else:
        arguments += ["--port", str(port)]
    if short_source_ids:
        arguments.append("--short-source-ids")
    return {"executable":str(Path(sys.executable).resolve()),
            "executable_sha256":hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest(),
            "arguments":arguments,
            "model_revision":model, "approved_by":approved_by, "scratch_root":str(scratch)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int)
    parser.add_argument("--max-output-tokens", type=int, default=1024)
    parser.add_argument("--write-config", type=Path)
    parser.add_argument("--size", choices=["0.8b", "2b", "4b"], default="0.8b")
    parser.add_argument("--backend", choices=["transformers", "resident", "openai"])
    parser.add_argument("--short-source-ids", action="store_true",
                        help="experimental loopback-only short evidence IDs; translate back before native validation")
    parser.add_argument("--serve", action="store_true", help="explicitly load one local model and serve loopback requests until stopped")
    parser.add_argument("--model-dir", type=Path, help="existing local Hugging Face snapshot; no automatic downloads")
    parser.add_argument("--device", choices=["auto", "cpu", "cuda"], default="auto",
                        help="direct backend device; explicit cuda refuses CPU fallback")
    parser.add_argument("--scratch-root", type=Path)
    parser.add_argument("--approved-by")
    args = parser.parse_args()
    backend = args.backend or ("resident" if args.write_config else "transformers" if args.model_dir else "openai")
    if args.short_source_ids and (backend != "openai" or args.serve):
        parser.error("--short-source-ids requires --backend openai and cannot serve a model")
    port = args.port if args.port is not None else (8000 if args.write_config or args.serve else 8080)
    if not 1 <= port <= 65535 or not 1 <= args.max_output_tokens <= 8192:
        parser.error("invalid port or output allowance")
    try:
        if args.serve:
            if args.write_config or args.model_dir is None:
                parser.error("--serve requires --model-dir and cannot write approval")
            with resident_server(args.model_dir, args.size, port, args.max_output_tokens, args.device) as server:
                print("resident summarizer ready", flush=True)
                server.serve_forever()
        elif args.write_config:
            if args.scratch_root is None or not args.approved_by:
                parser.error("--write-config requires --scratch-root and --approved-by")
            config = approval_config(args.size, backend, port, args.scratch_root, args.approved_by, args.model_dir, args.max_output_tokens, args.device, args.short_source_ids)
            # Never silently replace an existing approval or download/switch models.
            with args.write_config.open("x", encoding="utf-8") as output:
                output.write(json.dumps(config, indent=2) + "\n")
        elif backend == "transformers":
            if args.model_dir is None:
                parser.error("--backend transformers requires --model-dir")
            run_transformers(args.model_dir, args.max_output_tokens, args.device)
        elif backend == "resident":
            run_resident(port, args.max_output_tokens)
        else:
            run(port, args.max_output_tokens, args.short_source_ids)
    except Exception:
        # Never log source data, rejected candidate or server error bodies.
        raise SystemExit(1) from None
