"""Explicit dependency setup; imports and fits never provision."""
import argparse
import importlib.metadata
import json
import os
from pathlib import Path
import subprocess
import sys
import venv
from . import __version__
from .integrity import sha, source_digest
from .ettin import inspect_bundle, ENVIRONMENT


def dependency_commands(python, device):
    if device not in ('cpu','cuda:0'): raise ValueError('explicit_device_required')
    suffix, index = ('cpu','cpu') if device=='cpu' else ('cu128','cu128')
    base = [str(python), '-I', '-m', 'pip', 'install', '--disable-pip-version-check']
    return [base + ['--index-url','https://download.pytorch.org/whl/'+index,'torch==2.11.0+'+suffix],
            base + ['--index-url','https://pypi.org/simple'] + [name+'=='+version for name,version in ENVIRONMENT.items() if name!='torch']]


def provision(bundle, manifest_sha256, device, config_out, approved_by, scratch_root):
    output=Path(config_out)
    scratch=Path(scratch_root)
    if not output.is_absolute() or not scratch.is_absolute() or not scratch.is_dir() or not approved_by.strip():
        raise ValueError('absolute_destination_and_owned_scratch_required')
    environment=output.with_name(output.name+'.venv')
    # Atomic mkdir reserves this destination. Never replace an existing environment/approval.
    if output.exists(): raise FileExistsError('approval_exists_use_new_destination')
    manifest=inspect_bundle(bundle,manifest_sha256)
    environment.mkdir()
    venv.EnvBuilder(with_pip=True).create(environment)
    python=environment/('Scripts/python.exe' if os.name=='nt' else 'bin/python')
    for command in dependency_commands(python,device):
        subprocess.run(command,check=True,timeout=1200,env=provision_environment())
    expected=dict(ENVIRONMENT)
    expected['torch']='2.11.0+cpu' if device=='cpu' else '2.11.0+cu128'
    check="import importlib.metadata,json; print(json.dumps({n:importlib.metadata.version(n) for n in "+repr(list(expected))+"}))"
    versions=json.loads(subprocess.run([str(python),'-I','-c',check],check=True,capture_output=True,text=True,timeout=30,env=provision_environment()).stdout)
    if versions!=expected: raise ValueError('exact_dependency_profile_required')
    config=dict(python_executable=str(python.resolve()),python_sha256=sha(python,64*1024*1024),
        runtime_source=str(Path(__file__).parent.resolve()),runtime_source_sha256=source_digest(),runtime_version=__version__,
        bundle=str(Path(bundle).resolve()),manifest_sha256=manifest_sha256,revision=manifest['revision'],device=device,
        precision='fp32' if device=='cpu' else 'fp32-parameters-bf16-autocast',approved_by=approved_by,
        scratch_root=str(scratch.resolve()),startup_timeout_ms=120000,inference_timeout_ms=5000,cleanup_timeout_ms=1000)
    atomic_approval(output,config)
    return config


def provision_environment():
    # No provider credentials, user pip config, proxies or inherited Python injection.
    allowed=('SystemRoot','WINDIR','PATH','TEMP','TMP','HOME','USERPROFILE')
    env={k:os.environ[k] for k in allowed if k in os.environ}
    env.update(PIP_CONFIG_FILE=os.devnull,PIP_NO_INPUT='1')
    return env


def atomic_approval(path,config):
    path=Path(path)
    temporary=path.with_name(path.name+'.pending')
    with temporary.open('x',encoding='utf8') as stream:
        json.dump(config,stream,allow_nan=False,sort_keys=True)
        stream.flush()
        os.fsync(stream.fileno())
    try:
        # Hard link is atomic and refuses an existing destination (unlike replace).
        os.link(temporary,path)
    finally:
        temporary.unlink()


def main(arguments):
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--bundle',required=True);p.add_argument('--manifest-sha256',required=True)
    p.add_argument('--device',choices=('cpu','cuda:0'),required=True);p.add_argument('--config-out',required=True)
    p.add_argument('--approved-by',required=True);p.add_argument('--scratch-root',required=True)
    a=p.parse_args(arguments)
    provision(a.bundle,a.manifest_sha256,a.device,a.config_out,a.approved_by,a.scratch_root)
