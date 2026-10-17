"""Synthetic CPU-only adapter tests: never load or invoke an ONNX model."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import numpy as np

from tokenfold_select import onnx as implementation
from tokenfold_select.__main__ import configure
from tokenfold_select import protocol, _windows
import argparse

class Tokenizer:
    def no_truncation(self):pass
    def no_padding(self):pass
    def token_to_id(self,token):return {'[CLS]':1,'[SEP]':2}.get(token)
    def encode(self,text,add_special_tokens=False):return SimpleNamespace(ids=[3]*len(text))

class Adapter(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name).resolve();runtime=self.root/'runtime';runtime.mkdir()
        for source,target in ((Path(implementation.__file__),'onnx_score.py'),(Path(protocol.__file__),'protocol.py'),(Path(_windows.__file__),'packing.py')):
            shutil.copyfile(source,runtime/target)
        token=self.root/'tokenizer';token.mkdir()
        (token/'tokenizer.json').write_bytes(b'{}')
        (token/'tokenizer_config.json').write_text(json.dumps(dict(cls_token='[CLS]',sep_token='[SEP]')))
        (token/'special_tokens_map.json').write_bytes(b'{}')
        (self.root/'model.onnx').write_bytes(b'not-an-ONNX-graph-test-only')
        (self.root/'model.onnx.data').write_bytes(b'not-model-weights-test-only')
        self.m=implementation
        self.manifest=dict(status='development_ONNX_export',precision='fp32',provider='CPUExecutionProvider',opset=18,
            limits=dict(tokens=8192,groups=512,request_bytes=1048576,response_bytes=65536),
            source_checkpoint_sha256='a'*64,qualification=False,
            files={name:dict(bytes=(self.root/name).stat().st_size,sha256=self.m.digest(self.root/name)) for name in self.m.CAPS})
        self.pin=self.seal()

    def seal(self):
        p=self.root/'onnx-manifest.json';p.write_text(json.dumps(self.manifest))
        return self.m.digest(p)

    def request(self):return dict(schema_version=1,model_revision='a'*64,query='q',groups=[dict(id='first',text='abc'),dict(id='second',text='de')])

    def scorer(self):
        s=self.m.OnnxScorer(self.root,self.pin,development=True)
        s._tokenizer=Tokenizer();s._token_config=dict(cls_token='[CLS]',sep_token='[SEP]')
        return s

    def test_explicit_opt_in_and_trusted_manifest_required(self):
        with self.assertRaises(ValueError):self.m.OnnxScorer(self.root,self.pin)
        with self.assertRaises(ValueError):self.m.inspect_onnx(self.root,'0'*64)
        self.assertEqual(self.scorer().revision,'a'*64)

    def test_missing_or_tampered_external_data_refuses(self):
        p=self.root/'model.onnx.data';p.write_bytes(b'changed')
        with self.assertRaises(ValueError):self.scorer()
        p.unlink()
        with self.assertRaises(OSError):self.scorer()

    def test_unsupported_provider_and_incomplete_manifest_refuse(self):
        self.manifest['provider']='CUDAExecutionProvider';self.pin=self.seal()
        with self.assertRaises(ValueError):self.scorer()
        self.manifest['provider']='CPUExecutionProvider';self.manifest['files'].pop('model.onnx.data');self.pin=self.seal()
        with self.assertRaises(ValueError):self.scorer()

    def test_overflow_duplicate_ids_and_wrong_revision_refuse_before_session(self):
        s=self.scorer()
        with patch.object(self.m.importlib.metadata,'version',side_effect=lambda n:self.m.ENVIRONMENT[n]):
            request=self.request();request['groups'][0]['text']='x'*8192
            with self.assertRaises(ValueError):s.score(request)
            self.assertIsNone(s._session)
            request=self.request();request['groups'][1]['id']='first'
            with self.assertRaises(ValueError):s.score(request)
            request=self.request();request['model_revision']='b'*64
            with self.assertRaises(ValueError):s.score(request)

    def test_mock_scores_preserve_ids_and_nonfinite_or_shape_refuse(self):
        s=self.scorer();calls=[]
        class Session:
            invalid=None
            def run(self,names,inputs):
                calls.append(1)
                a=np.ones_like(inputs['input_ids'],dtype=np.float32)
                if self.invalid=='nan':a[0,0]=float('nan')
                return [a[:,:1] if self.invalid=='shape' else a]
        s._session=Session()
        with patch.object(self.m.importlib.metadata,'version',side_effect=lambda n:self.m.ENVIRONMENT[n]):
            self.assertEqual(s.score(self.request())['scores'],[['first',1.],['second',1.]])
            for kind in ('nan','shape'):
                s._session.invalid=kind
                with self.assertRaises(ValueError):s.score(self.request())
        self.assertEqual(len(calls),3)

    def test_wrong_runtime_version_refuses_without_loading(self):
        s=self.scorer()
        with patch.object(self.m.importlib.metadata,'version',return_value='unexpected'):
            with self.assertRaises(ValueError):s.score(self.request())
        self.assertIsNone(s._session)

    def test_wrong_provider_is_not_cached_or_silently_reused(self):
        s=self.scorer()
        session=SimpleNamespace(disable_fallback=lambda:None,get_providers=lambda:['CUDAExecutionProvider'])
        ort=SimpleNamespace(SessionOptions=SimpleNamespace,InferenceSession=lambda *_a,**_k:session)
        with patch.object(self.m.importlib.metadata,'version',side_effect=lambda n:self.m.ENVIRONMENT[n]),patch.dict(sys.modules,onnxruntime=ort):
            for _ in range(2):
                with self.assertRaisesRegex(ValueError,'explicit_CPU_provider'):s.score(self.request())
                self.assertIsNone(s._session)

    def test_installed_helpers_and_relative_paths_refuse(self):
        with self.assertRaisesRegex(ValueError,'absolute_root'):
            self.m.OnnxScorer(Path('relative'),self.pin,development=True)
        self.manifest['files']['runtime/packing.py']['sha256']='0'*64
        self.pin=self.seal()
        with self.assertRaises(ValueError):self.scorer()

    def test_configure_uses_native_interpreter_and_default_onnx_worker(self):
        args=argparse.Namespace(artifact=self.root,manifest_sha256=self.pin,revision='a'*64,
                               scratch_root=self.root,site_packages=self.root,approved_by='test-approval')
        config=configure(args)
        self.assertEqual(config['model_revision'],'a'*64)
        self.assertEqual(Path(config['executable']),Path(sys._base_executable).resolve())
        self.assertEqual(config['arguments'][:2],['-I','-S'])
        self.assertIn('--development',config['arguments'])
        self.assertNotIn('pytorch',config['arguments'])
        args.approved_by=' '
        with self.assertRaises(ValueError):configure(args)
        args.approved_by='test';args.revision='b'*64
        with self.assertRaises(ValueError):configure(args)

if __name__=='__main__':unittest.main()
