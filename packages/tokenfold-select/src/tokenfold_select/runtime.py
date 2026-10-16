"""Dedicated-process, explicitly opted-in development scoring; no text generation."""
import importlib.metadata
import json
import os
from pathlib import Path

from . import _windows
from .artifact import inspect_artifact
from .protocol import encode_response, validate_request

ENVIRONMENT = {
    'torch': '2.11.0+cu128', 'transformers': '5.19.0', 'safetensors': '0.8.0',
    'tokenizers': '0.23.2', 'huggingface_hub': '1.33.0', 'numpy': '2.5.3',
}


def initialize_windows_system_root():
    if os.name == 'nt':
        import ctypes
        buffer = ctypes.create_unicode_buffer(32768)
        get = ctypes.WinDLL('kernel32', use_last_error=True).GetWindowsDirectoryW
        get.argtypes = [ctypes.c_wchar_p, ctypes.c_uint]
        get.restype = ctypes.c_uint
        size = get(buffer, len(buffer))
        if not 0 < size < len(buffer):
            raise ValueError('windows_directory_unavailable')
        os.environ['SystemRoot'] = buffer.value


class DevelopmentScorer:
    """Single-window prototype. Use in its own process; GPU parity is still required."""

    def __init__(self, artifact, revision, *, cache_directory, development=False, precision='fp32'):
        if development is not True or precision not in ('fp32', 'fp16'):
            raise ValueError('explicit_development_precision_required')
        self.root = Path(artifact)
        self.report = inspect_artifact(self.root, revision)
        self.root = self.root.resolve()
        self.revision, self.precision = revision, precision
        self.cache = Path(cache_directory)
        if not self.cache.is_absolute() or self.cache.resolve().is_relative_to(self.root):
            raise ValueError('separate_absolute_cache_directory_required')
        self._tokenizer = self._model = self._torch = None
        self._token_config = None
        self.last_metrics = None

    def _prepare_environment(self):
        initialization = self.report.get('initialization_recipe')
        declared = initialization.get('environment', []) if isinstance(initialization, dict) else []
        if (not isinstance(declared, list) or len(declared) != len(ENVIRONMENT)
                or set(declared) != {name + '==' + value for name, value in ENVIRONMENT.items()}):
            raise ValueError('supported_provisioned_environment_required')
        for name, expected in ENVIRONMENT.items():
            if importlib.metadata.version(name) != expected:
                raise ValueError('supported_provisioned_environment_required')
        initialize_windows_system_root()
        self.cache.mkdir(parents=True, exist_ok=True)
        os.environ.update(HF_HUB_OFFLINE='1', TRANSFORMERS_OFFLINE='1',
                          HF_HOME=str(self.cache / 'hf'), TORCHINDUCTOR_CACHE_DIR=str(self.cache / 'torch'),
                          TOKENIZERS_PARALLELISM='false', CUBLAS_WORKSPACE_CONFIG=':4096:8')

    def _load_tokenizer(self):
        self._prepare_environment()
        from tokenizers import Tokenizer
        self._tokenizer = Tokenizer.from_file(str(self.root / 'tokenizer/tokenizer.json'))
        self._token_config = json.loads((self.root / 'tokenizer/tokenizer_config.json').read_bytes())

    def _load_model(self):
        self.report = inspect_artifact(self.root, self.revision)
        import torch
        if (not torch.cuda.is_available() or torch.cuda.get_device_capability() != (12, 0)
                or torch.cuda.get_device_name() != 'NVIDIA GeForce RTX 5080'):
            raise ValueError('tested_local_CUDA_target_required_no_fallback')
        from transformers.initialization import no_init_weights
        from transformers.models.modernbert.configuration_modernbert import ModernBertConfig
        from transformers.models.modernbert.modeling_modernbert import ModernBertModel
        from ._model import JointGroupModel
        torch.set_num_threads(4)
        torch.set_num_interop_threads(1)
        torch.use_deterministic_algorithms(True)
        torch.backends.cuda.matmul.allow_tf32 = False
        torch.backends.cudnn.allow_tf32 = False
        torch.backends.cudnn.benchmark = False
        config = ModernBertConfig.from_pretrained(self.root, local_files_only=True)
        config._attn_implementation = 'sdpa'
        with no_init_weights():
            model = JointGroupModel(ModernBertModel(config))
        state = torch.load(self.root / 'checkpoint.pt', map_location='cpu', weights_only=True, mmap=True)
        if any(not isinstance(value, torch.Tensor) or value.dtype != torch.float32
               or not torch.isfinite(value).all() for value in state.values()):
            raise ValueError('finite_FP32_source_tensors_required')
        model.load_state_dict(state, strict=True, assign=True)
        dtype = torch.float32 if self.precision == 'fp32' else torch.float16
        self._model = model.to(device='cuda', dtype=dtype).eval()
        self._torch = torch

    def score(self, request):
        """Admit the entire request before model creation, then one encoder forward."""
        self.last_metrics = None
        request = validate_request(request, self.revision)
        if self._tokenizer is None:
            self._load_tokenizer()
        windows, packing = _windows.pack_windows(self._tokenizer, self._token_config, request)
        if len(windows) != 1 or packing['context_tokens_per_window']:
            raise ValueError('single_complete_window_required_no_truncation')
        if self._model is None:
            self._load_model()
        torch = self._torch
        dtype = torch.float32 if self.precision == 'fp32' else torch.float16
        parameters = list(self._model.parameters())
        if not parameters or any(p.device.type != 'cuda' or p.dtype != dtype for p in parameters):
            raise ValueError('declared_CUDA_precision_required_no_fallback')
        ids, ranges, indices = windows[0]
        if indices != list(range(len(request['groups']))):
            raise ValueError('score_alignment')
        inputs = torch.tensor([ids], device='cuda')
        with torch.inference_mode():
            scores = self._model(inputs, torch.ones_like(inputs), ranges).cpu().tolist()
        encoded = encode_response(request, scores)
        self.last_metrics = dict(quality='unqualified', qualification=False, encoder_forwards=1,
                                 device='cuda', precision=self.precision, packing=packing)
        return json.loads(encoded)
