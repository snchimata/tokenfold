"""Bounded ONNX CPU reference scorer; no PyTorch, downloads or generated text."""
import hashlib
import importlib.metadata
import json
from pathlib import Path
import re

from . import _windows as packing
from . import protocol

CAPS={'model.onnx':64*1024**2,'model.onnx.data':1024**3,
      'tokenizer/tokenizer.json':32*1024**2,'tokenizer/tokenizer_config.json':65536,
      'tokenizer/special_tokens_map.json':65536,
      'runtime/onnx_score.py':65536,'runtime/packing.py':65536,'runtime/protocol.py':65536}
DEFAULT_REVISION='4dbdb5071e7d6316d8eb70dc161b1f2f77f555012f6616d7fa34311188e24cec'
DEFAULT_MANIFEST_SHA256='267313718e80aa91a56923dea30363fd5c24fde3782c716d6d048891860a2054'
ENVIRONMENT={'onnxruntime':'1.30.0','numpy':'2.5.3','tokenizers':'0.23.2'}

def digest(path):
    with Path(path).open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()

def inspect_onnx(directory,manifest_sha256=DEFAULT_MANIFEST_SHA256):
    root=Path(directory)
    if not root.is_absolute() or not re.fullmatch(r'[0-9a-f]{64}',manifest_sha256):
        raise ValueError('absolute_root_and_manifest_pin_required')
    root=root.resolve(strict=True);path=root/'onnx-manifest.json'
    if path.is_symlink() or path.stat().st_size>65536 or digest(path)!=manifest_sha256:
        raise ValueError('manifest_pin_mismatch')
    m=protocol.decode(path.read_bytes())
    if (m.get('status')!='development_ONNX_export' or m.get('precision')!='fp32'
        or m.get('provider')!='CPUExecutionProvider' or m.get('opset')!=18
        or m.get('limits')!={'tokens':8192,'groups':512,'request_bytes':1048576,'response_bytes':65536}
        or not re.fullmatch(r'[0-9a-f]{64}',m.get('source_checkpoint_sha256',''))
        or m.get('qualification') is not False or set(m.get('files',{}))!=set(CAPS)):
        raise ValueError('bounded_development_ONNX_profile_required')
    for name,cap in CAPS.items():
        path=root/name;pin=m['files'][name]
        if (path.is_symlink() or not path.resolve(strict=True).is_relative_to(root)
            or type(pin.get('bytes')) is not int or not 0<pin['bytes']<=cap
            or path.stat().st_size!=pin['bytes'] or digest(path)!=pin['sha256']):
            raise ValueError('executed_file_pin_mismatch')
    # Package code is installed locally, never imported from the model directory.
    for module,name in ((packing,'packing'),(protocol,'protocol')):
        if digest(module.__file__)!=m['files'][f'runtime/{name}.py']['sha256']:
            raise ValueError('installed_helpers_must_match_model')
    return m

class OnnxScorer:
    """One complete window; callers own process lifetime and final allocation."""
    def __init__(self,directory,manifest_sha256=DEFAULT_MANIFEST_SHA256,*,development=False):
        if development is not True:raise ValueError('explicit_development_opt_in_required')
        self.root=Path(directory)
        self.manifest_sha256=manifest_sha256
        self.manifest=inspect_onnx(self.root,manifest_sha256)
        self.revision=self.manifest['source_checkpoint_sha256']
        self._tokenizer=self._session=None

    def score(self,request):
        request=protocol.validate_request(request,self.revision)
        for name,version in ENVIRONMENT.items():
            if importlib.metadata.version(name)!=version:raise ValueError('pinned_ONNX_environment_required')
        if self._tokenizer is None:
            from tokenizers import Tokenizer
            self._tokenizer=Tokenizer.from_file(str(self.root/'tokenizer/tokenizer.json'))
            self._token_config=protocol.decode((self.root/'tokenizer/tokenizer_config.json').read_bytes())
        windows,receipt=packing.pack_windows(self._tokenizer,self._token_config,request)
        if len(windows)!=1 or receipt['context_tokens_per_window']:
            raise ValueError('single_complete_window_required_no_truncation')
        ids,ranges,indices=windows[0]
        if indices!=list(range(len(request['groups']))):raise ValueError('group_alignment')
        import numpy as np
        if self._session is None:
            inspect_onnx(self.root,self.manifest_sha256)
            import onnxruntime as ort
            options=ort.SessionOptions();options.intra_op_num_threads=4;options.inter_op_num_threads=1
            session=ort.InferenceSession(str(self.root/'model.onnx'),options,providers=['CPUExecutionProvider'])
            session.disable_fallback()
            if session.get_providers()!=['CPUExecutionProvider']:
                raise ValueError('explicit_CPU_provider_required')
            self._session=session
        inputs=np.asarray([ids],dtype=np.int64)
        output=self._session.run(['token_scores'],{'input_ids':inputs,'attention_mask':np.ones_like(inputs)})[0]
        if output.shape!=inputs.shape or not np.isfinite(output).all():raise ValueError('finite_token_scores_required')
        scores=[float(output[0,a:b].mean()) for a,b in ranges]
        return protocol.decode(protocol.encode_response(request,scores))
