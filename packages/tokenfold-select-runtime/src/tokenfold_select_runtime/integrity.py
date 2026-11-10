"""Bounded integrity and immutable approval helpers, no ML imports."""
import hashlib
import json
from pathlib import Path
import re
import sys
from . import __version__
from .protocol import decode

SOURCE_FILES = ('__init__.py', '__main__.py', 'ettin.py', 'integrity.py', 'protocol.py', 'provision.py', 'worker.py')

def sha(path, limit):
    digest = hashlib.sha256()
    total = 0
    with Path(path).open('rb') as stream:
        while chunk := stream.read(1024 * 1024):
            total += len(chunk)
            if total > limit: raise ValueError('artifact_byte_limit')
            digest.update(chunk)
    return digest.hexdigest()

def source_digest(directory=None):
    root = Path(directory or Path(__file__).parent).resolve(strict=True)
    if any(p.name not in SOURCE_FILES and p.name != "__pycache__" for p in root.iterdir()): raise ValueError("unapproved_source_entry")
    data = bytearray()
    for name in SOURCE_FILES:
        path = root / name
        if not path.resolve(strict=True).is_relative_to(root): raise ValueError('source_path_escape')
        with path.open("rb") as stream: raw = stream.read(256*1024+1)
        if len(raw) > 256*1024: raise ValueError('source_byte_limit')
        data.extend(name.encode() + b'\0' + raw + b'\0')
    return hashlib.sha256(data).hexdigest()

def validate_approval(config, *, inspect_model=True):
    from .ettin import inspect_bundle
    required = {'python_executable', 'python_sha256', 'runtime_source', 'runtime_source_sha256',
                'runtime_version', 'bundle', 'manifest_sha256', 'revision', 'device', 'precision',
                'approved_by', 'scratch_root', 'startup_timeout_ms', 'inference_timeout_ms', 'cleanup_timeout_ms'}
    if sys.version_info < (3,12): raise ValueError('python312_required')
    if not isinstance(config, dict) or set(config) != required: raise ValueError('invalid_approval')
    for key in ('python_executable', 'runtime_source', 'bundle', 'scratch_root'):
        if not isinstance(config[key], str) or not Path(config[key]).is_absolute(): raise ValueError('absolute_paths_required')
    for key in ('python_sha256','runtime_source_sha256','manifest_sha256','revision'):
        if not isinstance(config[key], str) or not re.fullmatch('[0-9a-f]{64}',config[key]): raise ValueError('exact_pin_required')
    if (config['runtime_version'] != __version__ or not isinstance(config['approved_by'],str)
            or not config['approved_by'].strip() or len(config['approved_by'].encode()) > 256
            or config['device'] not in ('cpu','cuda:0')
            or config['precision'] != ('fp32' if config['device']=='cpu' else 'fp32-parameters-bf16-autocast')):
        raise ValueError('approved_profile_required')
    for key,lo,hi in (('startup_timeout_ms',1,120000),('inference_timeout_ms',1000,30000),('cleanup_timeout_ms',1,1000)):
        if type(config[key]) is not int or not lo <= config[key] <= hi: raise ValueError('invalid_timeout')
    if Path(config['python_executable']).resolve(strict=True) != Path(sys.executable).resolve(strict=True): raise ValueError('python_path_mismatch')
    if sha(config['python_executable'],64*1024*1024) != config['python_sha256']: raise ValueError('python_pin_mismatch')
    if Path(config['runtime_source']).resolve(strict=True) != Path(__file__).parent.resolve(): raise ValueError('source_path_mismatch')
    if source_digest(config['runtime_source']) != config['runtime_source_sha256']: raise ValueError('source_pin_mismatch')
    if inspect_model:
        manifest=inspect_bundle(config['bundle'],config['manifest_sha256'])
        if manifest['revision'] != config['revision']: raise ValueError('revision_mismatch')
    return config
