"""Explicit local functional probe. Never installs dependencies or downloads weights."""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'src'))
from tokenfold_select_runtime.integrity import sha,source_digest


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--python',required=True);p.add_argument('--bundle',required=True)
    p.add_argument('--device',required=True,choices=('cpu','cuda:0'));p.add_argument('--scratch-root',required=True)
    a=p.parse_args();python=Path(a.python).resolve(strict=True);bundle=Path(a.bundle).resolve(strict=True)
    probe=subprocess.run([str(python),'-I','-B','-c',"import torch,json,importlib.metadata; print(json.dumps(dict(packages={n:importlib.metadata.version(n) for n in ['torch','transformers','tokenizers','safetensors','huggingface_hub']},cuda=torch.cuda.is_available(),bf16=torch.cuda.is_available() and torch.cuda.is_bf16_supported(),gpu=torch.cuda.get_device_name(0) if torch.cuda.is_available() else None)))"],capture_output=True,text=True,check=True)
    facts=json.loads(probe.stdout)
    print(json.dumps(dict(platform=platform.platform(),processor=platform.processor(),device=a.device,environment=facts,fresh_cpu_wheel_certification=False)),flush=True)
    if a.device=='cuda:0' and not(facts['cuda'] and facts['bf16']):
        print('SKIPPED: requested CUDA BF16 unavailable; no fallback');return
    source=Path(__file__).resolve().parents[1]/'src/tokenfold_select_runtime';manifest=bundle/'ettin-manifest.json'
    scratch=Path(a.scratch_root).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='functional-',dir=scratch) as directory:
        config=dict(python_executable=str(python),python_sha256=sha(python,64*1024*1024),runtime_source=str(source),runtime_source_sha256=source_digest(source),runtime_version='0.6.0',bundle=str(bundle),manifest_sha256=sha(manifest,65536),revision=json.loads(manifest.read_text())['revision'],device=a.device,precision='fp32' if a.device=='cpu' else 'fp32-parameters-bf16-autocast',approved_by='explicit local functional probe only',scratch_root=directory,startup_timeout_ms=120000,inference_timeout_ms=30000,cleanup_timeout_ms=1000)
        path=Path(directory)/'approval.json';path.write_text(json.dumps(config),encoding='utf8')
        env=dict(os.environ,TOKENFOLD_TEST_RUNTIME_CONFIG=str(path))
        subprocess.run(['cargo','test','-p','tokenfold-adapters','--locked','--test','select_runtime','real_approved_runtime_functional','--','--ignored','--nocapture'],env=env,check=True,timeout=180)

if __name__=='__main__':main()
