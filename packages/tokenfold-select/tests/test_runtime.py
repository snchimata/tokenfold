from contextlib import nullcontext
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import tokenfold_select
from tokenfold_select import DevelopmentScorer, inspect_artifact
from tokenfold_select import _windows, artifact, protocol, runtime

MODEL_SHA = 'a90cf1ed3274e4dcd9a968e2b51176abf8832bfa36076e07be1467d1c49a4d64'
WINDOW_SHA = 'fa879fb896e305a60ff10f3f5a6a23887da11b9741a210e2dc058154e148d929'


def write(path, value):
    path.write_text(json.dumps(value), encoding='utf8')


def fixture(root):
    """Synthetic bytes test metadata only; not a checkpoint or qualification."""
    root.mkdir()
    (root/'checkpoint.pt').write_bytes(b'synthetic-not-a-torch-checkpoint')
    revision = hashlib.sha256((root/'checkpoint.pt').read_bytes()).hexdigest()
    config = dict(model_type='modernbert', num_hidden_layers=12, hidden_size=768,
                  max_position_embeddings=8192,
                  layer_types=['full_attention' if i%3==0 else 'sliding_attention' for i in range(12)])
    write(root/'config.json', config)
    (root/'tokenizer').mkdir()
    pins = {}
    for name in artifact.TOKENIZER_FILES:
        (root/'tokenizer'/name).write_bytes(b'{}')
        pins[name] = hashlib.sha256(b'{}').hexdigest()
    report = dict(status='completed_development_only', checkpoint_sha256=revision,
        mode='supervised-foundation-feature-distilled', joint_model_code_sha256=MODEL_SHA,
        config_sha256=hashlib.sha256((root/'config.json').read_bytes()).hexdigest(),
        tokenizer_sha256=pins, initialization_recipe=dict(environment=[name+'=='+version for name,version in runtime.ENVIRONMENT.items()]),
        recipe=dict(objective_profile='optional-balanced-window-hardpair-headwarm-v4', precision='fp32',
                    student_token_cap=8192, scoring_window_profile=_windows.PROFILE,
                    scoring_context_token_cap=1536, scoring_window_code_sha256=WINDOW_SHA))
    write(root/'training-report.json', report)
    write(root/'DEVELOPMENT-DECISION.json', dict(checkpoint_sha256=revision, training_completed=True,
        completion_content_audit_passed=True, development_stop_go_passed=True,
        qualification=False, release_approved=False, redistribution_cleared=False))
    return revision


def request(revision):
    return dict(schema_version=1, model_revision=revision, query='find exact item',
                groups=[dict(id='a',text='exact café\r\n'),dict(id='b',text='other')])


class FakeTokenizer:
    def no_truncation(self):pass
    def no_padding(self):pass
    def token_to_id(self, token):return {'[CLS]':1,'[SEP]':2}.get(token)
    def encode(self, text, add_special_tokens=False):return SimpleNamespace(ids=[3]*len(text))


class Contracts(unittest.TestCase):
    def test_canonical_sources_and_import_have_no_framework_dependency(self):
        base = Path(tokenfold_select.__file__).parent
        self.assertEqual(artifact.sha(base/'_model.py',128*1024),MODEL_SHA)
        self.assertEqual(artifact.sha(base/'_windows.py',128*1024),WINDOW_SHA)
        script = 'import sys; sys.path.insert(0,sys.argv[1]); import tokenfold_select; assert not {"torch","transformers","tokenizers"} & set(sys.modules)'
        child = subprocess.run([sys._base_executable,'-I','-S','-c',script,str(base.parent)],capture_output=True)
        self.assertEqual(child.returncode,0,child.stderr)

    def test_schema_and_finite_response_preserve_order_and_only_return_scores(self):
        item = request('a'*64)
        parsed = protocol.load_request(json.dumps(item).encode(),'a'*64)
        answer = protocol.decode(protocol.encode_response(parsed,[1.,-2.]))
        self.assertEqual(answer,dict(schema_version=1,model_revision='a'*64,scores=[['a',1.],['b',-2.]]))
        self.assertNotIn('query',answer)
        for scores in ([1.],[True,0],[float('inf'),0],[float('nan'),0]):
            with self.assertRaises(ValueError):protocol.encode_response(parsed,scores)

    def test_duplicate_nonfinite_bad_membership_revision_and_caps_refuse(self):
        for data in (b'{"x":1,"x":2}',b'NaN',b'1e999',b'\xff'):
            with self.assertRaises((ValueError,UnicodeError)):protocol.load_request(data,'a'*64)
        with self.assertRaises(ValueError):protocol.load_request(b' '*(protocol.REQUEST_BYTES+1),'a'*64)
        for transform in (lambda d:d.update(schema_version=True),lambda d:d.update(model_revision='wrong'),
                          lambda d:d['groups'][1].update(id='a'),lambda d:d['groups'][0].update(gold=1),
                          lambda d:d.update(query=' '),lambda d:d.update(query='x'*4097)):
            item=request('a'*64);transform(item)
            with self.assertRaises(ValueError):protocol.validate_request(item,'a'*64)
        item=request('a'*64);item['groups']=[dict(id=str(i)+'x'*240,text='t') for i in range(512)]
        with self.assertRaisesRegex(ValueError,'response_byte_limit'):
            protocol.encode_response(item,[0.]*512)

    def test_artifact_inspection_is_integrity_only_and_rejects_content_drift(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'artifact';revision=fixture(root)
            self.assertEqual(inspect_artifact(root,revision)['checkpoint_sha256'],revision)
            self.assertNotIn('torch',sys.modules)
            (root/'tokenizer/tokenizer.json').write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError,'tokenizer_hash'):inspect_artifact(root,revision)
            (root/'tokenizer/tokenizer.json').write_bytes(b'{}')
            (root/'checkpoint.pt').write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError,'weights_hash'):inspect_artifact(root,revision)

    def test_failed_decision_unfinished_artifact_and_parent_paths_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'artifact';revision=fixture(root)
            decision=json.loads((root/'DEVELOPMENT-DECISION.json').read_bytes())
            decision['development_stop_go_passed']=False;write(root/'DEVELOPMENT-DECISION.json',decision)
            with self.assertRaisesRegex(ValueError,'accepted_development'):inspect_artifact(root,revision)
            with self.assertRaisesRegex(ValueError,'absolute'):inspect_artifact(Path('relative'),revision)
            with self.assertRaisesRegex(ValueError,'SHA256'):inspect_artifact(root,'auto')
            report=json.loads((root/'training-report.json').read_bytes());report['status']='running'
            write(root/'training-report.json',report)
            with self.assertRaisesRegex(ValueError,'completed_clean'):inspect_artifact(root,revision)
            # Pure path checker also refuses existing files outside the root.
            outside=Path(directory)/'outside';outside.write_bytes(b'test')
            with self.assertRaisesRegex(ValueError,'escape'):artifact._file(root,'../outside')

    def test_explicit_opt_in_separate_cache_and_whole_input_admission_before_model(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'artifact';revision=fixture(root)
            with self.assertRaises(ValueError):DevelopmentScorer(root,revision,cache_directory=Path(directory)/'cache')
            with self.assertRaises(ValueError):DevelopmentScorer(root,revision,cache_directory=root/'cache',development=True)
            scorer=DevelopmentScorer(root,revision,cache_directory=Path(directory)/'cache',development=True)
            scorer._tokenizer=FakeTokenizer();scorer._token_config=dict(cls_token='[CLS]',sep_token='[SEP]')
            item=request(revision);item['groups']=[dict(id=str(i),text='x'*5000) for i in range(2)]
            with patch.object(scorer,'_load_model',side_effect=AssertionError('no model should load')):
                with self.assertRaisesRegex(ValueError,'single_complete_window'):
                    scorer.score(item)
                item['groups']=[dict(id='big',text='x'*9000)]
                with self.assertRaisesRegex(ValueError,'whole_group_token_cap'):
                    scorer.score(item)
            self.assertIsNone(scorer.last_metrics)
            self.assertNotIn('torch',sys.modules)

    def test_mock_only_resident_scores_repeat_without_result_cache_and_refuse_wrong_dtype(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'artifact';revision=fixture(root)
            scorer=DevelopmentScorer(root,revision,cache_directory=Path(directory)/'cache',development=True)
            scorer._tokenizer=FakeTokenizer();scorer._token_config=dict(cls_token='[CLS]',sep_token='[SEP]')
            parameter=SimpleNamespace(device=SimpleNamespace(type='cuda'),dtype='float32')
            calls=[]
            class FakeModel:
                def parameters(self):return [parameter]
                def __call__(self,inputs,mask,ranges):
                    calls.append((inputs,ranges))
                    return SimpleNamespace(cpu=lambda:SimpleNamespace(tolist=lambda:[.5,-.5]))
            scorer._model=FakeModel()
            scorer._torch=SimpleNamespace(float32='float32',float16='float16',
                tensor=lambda x,device:x,ones_like=lambda x:x,inference_mode=nullcontext)
            first=scorer.score(request(revision));second=scorer.score(request(revision))
            self.assertEqual(first,second);self.assertEqual(len(calls),2)
            self.assertEqual(scorer.last_metrics['encoder_forwards'],1)
            parameter.dtype='float16'
            with self.assertRaisesRegex(ValueError,'precision'):scorer.score(request(revision))
            self.assertEqual(len(calls),2);self.assertIsNone(scorer.last_metrics)

    def test_native_bootstrap_inspection_and_refusal_have_no_model_or_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'artifact';revision=fixture(root)
            bootstrap=Path(tokenfold_select.__file__).with_name('bootstrap.py')
            # Native base interpreter loads only this installed package directory.
            site=Path(tokenfold_select.__file__).parents[1]
            command=[sys._base_executable,'-I','-S',str(bootstrap),'--site-packages',str(site)]
            child=subprocess.run(command+['inspect','--artifact',str(root),'--revision',revision],
                env={},capture_output=True,cwd=directory)
            self.assertEqual(child.returncode,0,child.stderr)
            self.assertFalse(json.loads(child.stdout)['checkpoint_tensors_loaded'])
            paths=Path(directory)/'request.json',Path(directory)/'response.json'
            write(paths[0],request(revision))
            child=subprocess.run(command+['worker','--backend','pytorch','--artifact',str(root),'--revision','0'*64,'--development'],
                env=dict(TOKENFOLD_SELECT_REQUEST_PATH=str(paths[0]),TOKENFOLD_SELECT_RESPONSE_PATH=str(paths[1])),
                capture_output=True,cwd=directory,timeout=10)
            self.assertEqual(child.returncode,1)
            self.assertEqual(child.stdout,b'')
            self.assertNotIn(b'find exact',child.stderr)
            self.assertFalse(paths[1].exists())


if __name__ == '__main__':unittest.main()
