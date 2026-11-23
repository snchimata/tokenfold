"""Explicit, unqualified Ettin Select-v3 scorer for a dedicated owned process.

No downloads, trainer imports, base-weight loading, generation or CPU fallback.
512-group/multiwindow admission is an experimental inference policy, not evidence
that the development fit covered those shapes. Existing v2 APIs are unchanged.
"""
import importlib.metadata
import json
import os
from pathlib import Path
import re

from .integrity import sha
from .protocol import RESPONSE_BYTES, decode, encode_response, validate_request

PROFILE = 'ettin7999-whole-group-query-repeat-v1'
CONTEXT = 7999
MAX_WINDOWS = 64
MAX_TOTAL_TOKENS = MAX_WINDOWS * CONTEXT
LIMITS = dict(tokens=CONTEXT, groups=512, windows=MAX_WINDOWS,
              total_tokens=MAX_TOTAL_TOKENS, request_bytes=1048576, response_bytes=65536)
CAPS = {'model.safetensors': 1024**3, 'config.json': 65536,
        'tokenizer.json': 32*1024**2, 'tokenizer_config.json': 65536}
ENVIRONMENT = {'torch': '2.11.0+cu128', 'transformers': '5.19.0', 'safetensors': '0.8.0',
               'tokenizers': '0.23.2', 'huggingface_hub': '1.33.0'}


def inspect_bundle(directory, manifest_sha256):
    """Caller-pinned, confined byte/geometry inspection, never rights/quality proof."""
    root = Path(directory)
    if not root.is_absolute() or not isinstance(manifest_sha256, str) or not re.fullmatch('[0-9a-f]{64}', manifest_sha256):
        raise ValueError('absolute_bundle_and_exact_manifest_pin_required')
    root = root.resolve(strict=True)
    path = root / 'ettin-manifest.json'
    if not path.resolve(strict=True).is_relative_to(root) or sha(path, 65536) != manifest_sha256:
        raise ValueError('ettin_manifest_pin_mismatch')
    manifest = decode(path.read_bytes())
    if (not isinstance(manifest, dict)
            or manifest.get('kind') != 'tokenfold-select-v3-ettin-joint-development-bundle-v1'
            or manifest.get('profile') != PROFILE or manifest.get('limits') != LIMITS
            or manifest.get('base_model') != dict(repository='cross-encoder/ettin-reranker-150m-v1',
                revision='025501c4e0f9bbeb4c5b198318e0089ff061cc14')
            or manifest.get('architecture') != 'EttinJointGroupMeanNonlinear-v1'
            or manifest.get('trained_group_cap') != 24
            or manifest.get('qualification') is not False or manifest.get('publication_authorized') is not False
            or not isinstance(manifest.get('revision'), str) or not re.fullmatch('[0-9a-f]{64}', manifest['revision'])
            or not isinstance(manifest.get('files'), dict) or set(manifest['files']) != set(CAPS)):
        raise ValueError('exact_unqualified_ettin_bundle_profile_required')
    for name, cap in CAPS.items():
        path = root / name
        pin = manifest['files'][name]
        if (not path.resolve(strict=True).is_relative_to(root) or not isinstance(pin, dict)
                or set(pin) != {'bytes', 'sha256'}
                or type(pin['bytes']) is not int or not 0 < pin['bytes'] <= cap
                or path.stat().st_size != pin['bytes'] or sha(path, cap) != pin['sha256']):
            raise ValueError('ettin_bundle_file_pin_mismatch')
    if manifest['files']['model.safetensors']['sha256'] != manifest['revision']:
        raise ValueError('checkpoint_revision_mismatch')
    config = decode((root / 'config.json').read_bytes())
    if (not isinstance(config, dict) or config.get('architectures') != ['ModernBertModel'] or config.get('hidden_size') != 768
            or config.get('num_hidden_layers') != 22 or config.get('max_position_embeddings') != CONTEXT
            or config.get('vocab_size') != 50368):
        raise ValueError('complete_original_ettin_geometry_required')
    return manifest


def pack_windows(tokenizer, request, revision):
    """Admit all original groups/windows BEFORE any model construction/forward."""
    request = validate_request(request, revision)
    # Conservative finite-FP32 JSON score allowance: reject before inference,
    # not after spending GPU work on a response that cannot fit the protocol.
    if len(encode_response(request, [0.0] * len(request['groups']))) + 32*len(request['groups']) > RESPONSE_BYTES:
        raise ValueError('response_capacity_preflight')
    tokenizer.no_truncation()
    tokenizer.no_padding()
    if tokenizer.token_to_id('[CLS]') != 50281 or tokenizer.token_to_id('[SEP]') != 50282:
        raise ValueError('native_ettin_special_tokens_required')

    def encode(text):
        ids = tokenizer.encode(text, add_special_tokens=False).ids
        if not ids or any(type(token) is not int or not 0 <= token < 50368 for token in ids):
            raise ValueError('complete_nonempty_token_content_required')
        return ids

    query = [50281] + encode(request['query']) + [50282]
    if len(query) + 2 > CONTEXT:
        raise ValueError('query_context_cap')
    content = [encode(group['text']) for group in request['groups']]
    if any(len(query) + len(ids) + 1 > CONTEXT for ids in content):
        raise ValueError('whole_group_context_cap')
    windows = []
    ids, ranges, indices = list(query), [], []
    for index, tokens in enumerate(content):
        if len(ids) + len(tokens) + 1 > CONTEXT:
            windows.append((ids, ranges, indices))
            ids, ranges, indices = list(query), [], []
        start = len(ids)
        ids.extend(tokens)
        ranges.append((start, len(ids)))
        ids.append(50282)
        indices.append(index)
    windows.append((ids, ranges, indices))
    total = sum(len(window[0]) for window in windows)
    if len(windows) > MAX_WINDOWS or total > MAX_TOTAL_TOKENS:
        raise ValueError('whole_request_window_or_total_cap')
    if [index for _, _, indices in windows for index in indices] != list(range(len(content))):
        raise ValueError('complete_original_group_alignment_required')
    return windows, dict(profile=PROFILE, windows=len(windows), groups=len(content),
        total_encoded_tokens=total, max_window_tokens=max(len(window[0]) for window in windows),
        context_copies=0, outside_training_group_counts=any(not 2 <= len(w[1]) <= 24 for w in windows),
        cross_window_context_unqualified=len(windows) > 1, qualification=False)


class EttinDevelopmentScorer:
    """Concurrency one; caller owns original process lifetime/deadline/memory guards."""

    def __init__(self, directory, manifest_sha256, *, development=False, precision=None, device='cuda:0'):
        if (development is not True or device not in ('cpu', 'cuda:0')
                or precision != ('fp32' if device == 'cpu' else 'fp32-parameters-bf16-autocast')):
            raise ValueError('explicit_ettin_development_precision_required')
        self.device = device
        self.root = Path(directory).resolve()
        self.pin = manifest_sha256
        self.manifest = inspect_bundle(directory, manifest_sha256)
        self.revision = self.manifest['revision']
        self._tokenizer = self._model = self._torch = None
        self.last_receipt = None

    def _load(self):
        cuda = self.device == 'cuda:0'
        required = (('HF_HUB_OFFLINE', '1'), ('TRANSFORMERS_OFFLINE', '1'))
        if cuda:
            required += (('CUDA_VISIBLE_DEVICES', '0'),)
        if any(os.environ.get(key) != value for key, value in required):
            raise ValueError('explicit_cuda0_offline_environment_required' if cuda
                             else 'explicit_cpu_offline_environment_required')
        for name, expected in ENVIRONMENT.items():
            actual = importlib.metadata.version(name)
            if actual != expected and not (name == 'torch' and not cuda and actual in ('2.11.0', '2.11.0+cpu')):
                raise ValueError('exact_ettin_runtime_environment_required')
        inspect_bundle(self.root, self.pin)
        import torch
        from safetensors.torch import load_file
        from transformers.initialization import no_init_weights
        from transformers.models.modernbert.configuration_modernbert import ModernBertConfig
        from transformers.models.modernbert.modeling_modernbert import ModernBertModel
        if cuda:
            if not torch.cuda.is_available() or not torch.cuda.is_bf16_supported():
                raise ValueError('cuda_bf16_required_no_fallback')
            torch.cuda.set_device(0)
        torch.set_num_threads(1)
        if torch.get_num_interop_threads() != 1:
            torch.set_num_interop_threads(1)
        if cuda:
            torch.set_float32_matmul_precision('highest')
            torch.backends.cuda.matmul.allow_tf32 = False
            torch.backends.cudnn.allow_tf32 = False
        config = ModernBertConfig.from_pretrained(self.root, local_files_only=True)
        config._attn_implementation = 'sdpa'

        class Model(torch.nn.Module):
            def __init__(self):
                super().__init__()
                self.encoder = ModernBertModel(config)
                self.head = torch.nn.Sequential(torch.nn.Linear(768, 768, bias=False), torch.nn.GELU(),
                    torch.nn.LayerNorm(768, eps=1e-5), torch.nn.Linear(768, 1, bias=True))

            def forward(self, input_ids, attention_mask, ranges):
                if (input_ids.ndim != 2 or input_ids.shape[0] != 1
                        or attention_mask.shape != input_ids.shape or input_ids.shape[1] > CONTEXT
                        or not 1 <= len(ranges) <= 512 or not attention_mask.all()):
                    raise ValueError('complete_single_joint_window_required')
                previous = 0
                for start, end in ranges:
                    if not previous <= start < end <= input_ids.shape[1]:
                        raise ValueError('ordered_nonempty_content_ranges_required')
                    previous = end
                states = self.encoder(input_ids=input_ids, attention_mask=attention_mask).last_hidden_state[0]
                pooled = torch.stack([states[start:end].mean(0) for start, end in ranges])
                return self.head(pooled).squeeze(-1)

        with no_init_weights():
            model = Model()
        model.load_state_dict(load_file(str(self.root / 'model.safetensors'), device='cpu'), strict=True, assign=True)
        if sum(p.numel() for p in model.parameters()) != 149606401 or any(p.dtype != torch.float32 for p in model.parameters()):
            raise ValueError('complete_fp32_ettin_checkpoint_required')
        model.to(self.device).eval()
        if any(p.device != torch.device(self.device) for p in model.parameters()):
            raise ValueError('actual_cuda0_placement_required' if cuda else 'actual_cpu_placement_required')
        self._torch, self._model = torch, model

    def admit(self, request):
        request = validate_request(request, self.revision)
        if self._tokenizer is None:
            from tokenizers import Tokenizer
            self._tokenizer = Tokenizer.from_file(str(self.root / 'tokenizer.json'))
        windows, receipt = pack_windows(self._tokenizer, request, self.revision)
        return windows, receipt

    def score_admitted(self, request, windows, receipt):
        if self._model is None:
            self._load()
        torch = self._torch
        result = []
        with torch.inference_mode():
            for ids, ranges, indices in windows:
                inputs = torch.tensor([ids], dtype=torch.long, device=self.device)
                with (torch.autocast('cuda', dtype=torch.bfloat16, cache_enabled=False) if self.device == 'cuda:0'
                      else torch.autocast('cpu', enabled=False)):
                    scores = self._model(inputs, torch.ones_like(inputs), ranges)
                if scores.ndim != 1 or len(scores) != len(indices) or not torch.isfinite(scores).all():
                    raise ValueError('finite_aligned_ettin_scores_required')
                if self.device == 'cuda:0':
                    torch.cuda.synchronize(0)
                result.extend(scores.float().cpu().tolist())
        response = decode(encode_response(request, result))
        self.last_receipt = receipt
        return response

    def score(self, request):
        windows, receipt = self.admit(request)
        return self.score_admitted(request, windows, receipt)
