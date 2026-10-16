"""Portable integrity/development admission, never a model or rights certificate."""
import hashlib
from pathlib import Path

from . import _windows
from .protocol import decode

TOKENIZER_FILES = {'tokenizer.json', 'tokenizer_config.json', 'special_tokens_map.json'}


def sha(path, limit):
    total = 0
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        while chunk := stream.read(1024 * 1024):
            total += len(chunk)
            if total > limit:
                raise ValueError('artifact_byte_limit')
            digest.update(chunk)
    return digest.hexdigest()


def _file(root, name):
    path = (root / name).resolve(strict=True)
    if not path.is_relative_to(root) or not path.is_file():
        raise ValueError('artifact_path_escape')
    return path


def _json(root, name, limit):
    path = _file(root, name)
    with path.open('rb') as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError('artifact_byte_limit')
    return decode(data)


def inspect_artifact(directory, revision):
    """Check bytes/declared completion only, with no imports of ML frameworks."""
    if (not isinstance(revision, str) or len(revision) != 64
            or any(c not in '0123456789abcdef' for c in revision)):
        raise ValueError('checkpoint_SHA256_required')
    root = Path(directory)
    if not root.is_absolute():
        raise ValueError('absolute_artifact_path_required')
    root = root.resolve(strict=True)
    report = _json(root, 'training-report.json', 1024 * 1024)
    if (not isinstance(report, dict) or not isinstance(report.get('recipe'), dict)
            or report.get('status') != 'completed_development_only'
            or report.get('checkpoint_sha256') != revision
            or report.get('mode') != 'supervised-foundation-feature-distilled'
            or report.get('recipe', {}).get('objective_profile') != 'optional-balanced-window-hardpair-headwarm-v4'
            or report.get('recipe', {}).get('precision') != 'fp32'
            or report.get('recipe', {}).get('student_token_cap') != 8192):
        raise ValueError('completed_clean_compact12_development_artifact_required')
    _windows.verify_profile(report)
    if sha(Path(__file__).with_name('_model.py'), 128 * 1024) != report.get('joint_model_code_sha256'):
        raise ValueError('model_code_hash_mismatch')
    config = _json(root, 'config.json', 65536)
    if (not isinstance(config, dict) or config.get('model_type') != 'modernbert' or config.get('num_hidden_layers') != 12
            or config.get('hidden_size') != 768 or config.get('max_position_embeddings') != 8192
            or config.get('layer_types') != ['full_attention' if i % 3 == 0 else 'sliding_attention' for i in range(12)]):
        raise ValueError('compact12_configuration_required')
    if sha(_file(root, 'config.json'), 65536) != report.get('config_sha256'):
        raise ValueError('config_hash_mismatch')
    if sha(_file(root, 'checkpoint.pt'), 1024**3) != revision:
        raise ValueError('weights_hash_mismatch')
    pins = report.get('tokenizer_sha256')
    if not isinstance(pins, dict) or set(pins) != TOKENIZER_FILES:
        raise ValueError('complete_tokenizer_pins_required')
    if {p.name for p in (root / 'tokenizer').iterdir()} != TOKENIZER_FILES:
        raise ValueError('tokenizer_files_mismatch')
    for name, digest in pins.items():
        if sha(_file(root, 'tokenizer/' + name), 32 * 1024**2) != digest:
            raise ValueError('tokenizer_hash_mismatch')
    decision = _json(root, 'DEVELOPMENT-DECISION.json', 1024 * 1024)
    if (not isinstance(decision, dict) or decision.get('checkpoint_sha256') != revision
            or any(decision.get(key) is not True for key in
                   ('training_completed', 'completion_content_audit_passed', 'development_stop_go_passed'))
            or any(decision.get(key) is not False for key in ('qualification', 'release_approved', 'redistribution_cleared'))):
        raise ValueError('accepted_development_decision_required_not_release')
    return report
