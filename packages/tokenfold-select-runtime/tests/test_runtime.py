import sys
from pathlib import Path
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from tokenfold_select_runtime import __version__
from tokenfold_select_runtime.provision import dependency_commands
from tokenfold_select_runtime.protocol import decode, validate_request

class RuntimeTests(unittest.TestCase):
    def test_version_and_exact_profiles(self):
        self.assertEqual(__version__, '0.6.0')
        cpu = dependency_commands('python', 'cpu')
        cuda = dependency_commands('python', 'cuda:0')
        self.assertIn('torch==2.11.0+cpu', cpu[0])
        self.assertIn('https://download.pytorch.org/whl/cpu', cpu[0])
        self.assertIn('torch==2.11.0+cu128', cuda[0])
        self.assertIn('https://download.pytorch.org/whl/cu128', cuda[0])
        self.assertIn('transformers==5.19.0', cpu[1])
        self.assertNotIn('--extra-index-url', cpu[0])
    def test_protocol_equivalence_and_strict_json(self):
        request = dict(schema_version=1, model_revision='a'*64, query='query', groups=[dict(id='a',text='text')])
        self.assertEqual(validate_request(request, 'a'*64), request)
        for raw in (b'{"a":1,"a":2}', b'{"a":NaN}', b'{"a":1e999}'):
            with self.assertRaises(ValueError): decode(raw)
        request['groups'] *= 2
        with self.assertRaises(ValueError): validate_request(request, 'a'*64)

import io
import json
import tempfile
import threading
from tokenfold_select_runtime import worker
from tokenfold_select_runtime.integrity import source_digest, validate_approval
from tokenfold_select_runtime.provision import atomic_approval, provision

class ApprovalTests(unittest.TestCase):
    def test_atomic_approval_never_overwrites(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'approval.json'
            atomic_approval(path,{'x':1})
            self.assertEqual(json.loads(path.read_text()),{'x':1})
            with self.assertRaises(FileExistsError): atomic_approval(path,{'x':2})
            self.assertEqual(json.loads(path.read_text()),{'x':1})
            self.assertFalse(path.with_name(path.name+'.pending').exists())
    def test_provision_exact_commands_and_new_environment_reservation(self):
        for device in ('cpu','cuda:0'):
            with self.subTest(device=device),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);output=root/'approval.json';expected=dict(__import__('tokenfold_select_runtime.ettin',fromlist=['ENVIRONMENT']).ENVIRONMENT)
                expected['torch']='2.11.0+cpu' if device=='cpu' else '2.11.0+cu128'
                def create(environment):
                    executable=environment/('Scripts/python.exe' if __import__('os').name=='nt' else 'bin/python')
                    executable.parent.mkdir();executable.write_bytes(b'test-python')
                from types import SimpleNamespace
                with patch('tokenfold_select_runtime.provision.inspect_bundle',return_value={'revision':'a'*64}),patch('venv.EnvBuilder.create',side_effect=create),patch('subprocess.run',return_value=SimpleNamespace(stdout=json.dumps(expected))) as run:
                    config=provision(root,'b'*64,device,output,'human',root)
                    self.assertEqual(run.call_args_list[0].args[0],[config['python_executable'],'-I','-m','ensurepip','--upgrade','--default-pip'])
                    self.assertEqual(run.call_args_list[1].args[0],dependency_commands(config['python_executable'],device)[0])
                    self.assertEqual(run.call_args_list[2].args[0],dependency_commands(config['python_executable'],device)[1])
                    self.assertEqual(config['runtime_source_sha256'],source_digest())
                    self.assertEqual(json.loads(output.read_text()),config)
                    with self.assertRaises(FileExistsError): provision(root,'b'*64,device,output,'human',root)
                    self.assertEqual(run.call_count,4)
    def test_failed_setup_does_not_approve_or_reuse_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);output=root/'approval.json'
            with patch('tokenfold_select_runtime.provision.inspect_bundle',return_value={'revision':'a'*64}),patch('venv.EnvBuilder.create',side_effect=RuntimeError('failed')):
                with self.assertRaises(RuntimeError):provision(root,'b'*64,'cpu',output,'human',root)
                self.assertFalse(output.exists())
                with self.assertRaises(FileExistsError):provision(root,'b'*64,'cpu',output,'human',root)
    def test_changed_source_python_model_pins_are_rejected(self):
        import sys,hashlib
        from tokenfold_select_runtime.integrity import sha
        with tempfile.TemporaryDirectory() as directory:
            config=dict(python_executable=sys.executable,python_sha256=sha(sys.executable,64*1024*1024),runtime_source=str(Path(worker.__file__).parent),runtime_source_sha256=source_digest(),runtime_version='0.6.0',bundle=directory,manifest_sha256='a'*64,revision='b'*64,device='cpu',precision='fp32',approved_by='human',scratch_root=directory,startup_timeout_ms=120000,inference_timeout_ms=5000,cleanup_timeout_ms=1000)
            self.assertEqual(validate_approval(config,inspect_model=False),config)
            for key in ('python_sha256','runtime_source_sha256'):
                changed=dict(config);changed[key]='0'*64
                with self.assertRaises(ValueError):validate_approval(changed,inspect_model=False)
            with patch('tokenfold_select_runtime.ettin.inspect_bundle',return_value={'revision':'c'*64}):
                with self.assertRaisesRegex(ValueError,'revision_mismatch'):validate_approval(config)

class WorkerTests(unittest.TestCase):
    def frames(self,values):
        import struct
        raw=io.BytesIO()
        for id,op,body in values:
            data=json.dumps(dict(protocol_version=2,request_id=id,op=op,body=body)).encode();raw.write(struct.pack('<I',len(data))+data)
        raw.seek(0);return raw
    def responses(self,stream):
        import struct
        data=stream.getvalue();values=[]
        while data:
            n=struct.unpack('<I',data[:4])[0];values.append(json.loads(data[4:4+n]));data=data[4+n:]
        return values
    def test_admission_rejection_precedes_weight_loading(self):
        config={'bundle':'bundle','manifest_sha256':'pin','device':'cpu','precision':'fp32'}
        request=dict(schema_version=1,model_revision='a'*64,query='query',groups=[dict(id='g',text='whole')])
        with patch('tokenfold_select_runtime.worker.validate_approval',return_value=config),patch('tokenfold_select_runtime.ettin.EttinDevelopmentScorer') as scorer:
            scorer.return_value.revision='a'*64;scorer.return_value.admit.side_effect=ValueError('whole_group_context_cap')
            output=io.BytesIO();worker.serve(self.frames([(1,'load',dict(config=config,warmup=False)),(2,'score',request)]),output)
            scorer.return_value._load.assert_not_called()
            self.assertEqual([r['op'] for r in self.responses(output)],['ready','error'])
            self.assertEqual(self.responses(output)[1]['body'],dict(code='selection_limit'))
    def test_warmup_loads_and_score_reuses_without_implicit_retry(self):
        config={'bundle':'bundle','manifest_sha256':'pin','device':'cpu','precision':'fp32'}
        with patch('tokenfold_select_runtime.worker.validate_approval',return_value=config),patch('tokenfold_select_runtime.ettin.EttinDevelopmentScorer') as scorer:
            scorer.return_value._model=None
            scorer.return_value._load.side_effect=lambda:setattr(scorer.return_value,'_model',object())
            output=io.BytesIO();worker.serve(self.frames([(1,'load',dict(config=config,warmup=True)),(2,'load',dict(config=config,warmup=True)),(3,'close',{})]),output)
            scorer.return_value._load.assert_called_once()
            self.assertEqual([r['op'] for r in self.responses(output)],['ready','ready','closed'])
    def test_offline_audit_blocks_network_and_descendants(self):
        for event in ('socket.connect','socket.getaddrinfo','socket.bind','subprocess.Popen','os.system'):
            with self.assertRaises(ValueError):worker.offline(event,())
        worker.offline('open',())
    def test_frames_are_bounded_strict_and_monotonic(self):
        import struct
        for data in (struct.pack('<I',1048577),struct.pack('<I',0)):
            with self.assertRaises(ValueError):worker.read_frame(io.BytesIO(data))
        output=io.BytesIO();worker.serve(self.frames([(1,'close',{'extra':True})]),output)
        self.assertEqual(self.responses(output)[0]['body'],{'code':'failure'})

class PortEquivalenceTests(unittest.TestCase):
    def test_pinned_geometry_and_window_fixture(self):
        from tokenfold_select_runtime.ettin import pack_windows
        from types import SimpleNamespace
        class Tokenizer:
            def no_truncation(self):pass
            def no_padding(self):pass
            def token_to_id(self,token):return {'[CLS]':50281,'[SEP]':50282}[token]
            def encode(self,text,**kwargs):return SimpleNamespace(ids=[ord(c) for c in text])
        request=dict(schema_version=1,model_revision='a'*64,query='q',groups=[dict(id='a',text='ab'),dict(id='b',text='c')])
        windows,receipt=pack_windows(Tokenizer(),request,'a'*64)
        self.assertEqual(windows,[([50281,113,50282,97,98,50282,99,50282],[(3,5),(6,7)],[0,1])])
        self.assertFalse(receipt['outside_training_group_counts']);self.assertFalse(receipt['cross_window_context_unqualified'])
        request['groups'][0]['text']='x'*7999
        with self.assertRaisesRegex(ValueError,'whole_group_context_cap'):pack_windows(Tokenizer(),request,'a'*64)
class ProtocolControlTests(unittest.TestCase):
    def test_control_frames_are_limited_to_four_kib(self):
        import struct
        raw=json.dumps(dict(protocol_version=2,request_id=1,op='load',body={'padding':'x'*4096})).encode()
        with self.assertRaises(ValueError):worker.read_frame(io.BytesIO(struct.pack('<I',len(raw))+raw))
    def test_all_admitted_windows_preserve_whole_original_groups(self):
        from tokenfold_select_runtime.ettin import pack_windows
        from types import SimpleNamespace
        class Tokenizer:
            def no_truncation(self):pass
            def no_padding(self):pass
            def token_to_id(self,t):return {'[CLS]':50281,'[SEP]':50282}[t]
            def encode(self,t,**kw):return SimpleNamespace(ids=[42]*len(t))
        request=dict(schema_version=1,model_revision='a'*64,query='q',groups=[dict(id='a',text='x'*4000),dict(id='b',text='y'*4000)])
        windows,receipt=pack_windows(Tokenizer(),request,'a'*64)
        self.assertEqual([indices for _,_,indices in windows],[[0],[1]])
        self.assertEqual(receipt['total_encoded_tokens'],8008)
        self.assertTrue(receipt['outside_training_group_counts']);self.assertTrue(receipt['cross_window_context_unqualified'])
    def test_source_entries_cannot_hide_unpinned_extension_modules(self):
        import shutil
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            for name in __import__('tokenfold_select_runtime.integrity',fromlist=['SOURCE_FILES']).SOURCE_FILES:
                shutil.copy(Path(worker.__file__).parent/name,root/name)
            self.assertEqual(source_digest(root),source_digest())
            (root/'ettin.pyd').write_bytes(b'unapproved')
            with self.assertRaises(ValueError):source_digest(root)


class BundleIntegrityTests(unittest.TestCase):
    def test_model_manifest_geometry_and_source_pins_fail_closed(self):
        from tokenfold_select_runtime.ettin import inspect_bundle,PROFILE,LIMITS
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            config=dict(architectures=['ModernBertModel'],hidden_size=768,num_hidden_layers=22,max_position_embeddings=7999,vocab_size=50368)
            files={'model.safetensors':b'fixture-not-real-weights','config.json':json.dumps(config).encode(),'tokenizer.json':b'{}','tokenizer_config.json':b'{}'}
            pins={}
            for name,raw in files.items():
                (root/name).write_bytes(raw);pins[name]=dict(bytes=len(raw),sha256=hashlib.sha256(raw).hexdigest())
            manifest=dict(kind='tokenfold-select-v3-ettin-joint-development-bundle-v1',profile=PROFILE,limits=LIMITS,base_model=dict(repository='cross-encoder/ettin-reranker-150m-v1',revision='025501c4e0f9bbeb4c5b198318e0089ff061cc14'),architecture='EttinJointGroupMeanNonlinear-v1',trained_group_cap=24,qualification=False,publication_authorized=False,revision=pins['model.safetensors']['sha256'],files=pins)
            raw=json.dumps(manifest).encode();(root/'ettin-manifest.json').write_bytes(raw);pin=hashlib.sha256(raw).hexdigest()
            self.assertEqual(inspect_bundle(root,pin),manifest)
            with self.assertRaises(ValueError):inspect_bundle(root,'0'*64)
            (root/'model.safetensors').write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError,'ettin_bundle_file_pin_mismatch'):inspect_bundle(root,pin)


class ProvisionBootstrapIsolationTests(unittest.TestCase):
    def test_all_setup_children_include_sanitized_bounded_isolated_bootstrap(self):
        import os
        from types import SimpleNamespace
        scratch=Path(__file__).resolve().parents[3]/'.superpowers/sdd/sdk-updates/runtime-scratch'
        scratch.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='provision-fix-',dir=scratch) as directory:
            root=Path(directory);output=root/'approval.json';calls=[]
            expected=dict(__import__('tokenfold_select_runtime.ettin',fromlist=['ENVIRONMENT']).ENVIRONMENT);expected['torch']='2.11.0+cpu'
            def run(command,**kwargs):
                calls.append((command,kwargs));return SimpleNamespace(stdout=json.dumps(expected))
            def legacy_bootstrap(command,**kwargs):
                calls.append((command,kwargs));return b''
            with patch.dict(os.environ,{'OPENAI_API_KEY':'test-only-secret','HTTPS_PROXY':'http://test-only-proxy'}),patch('tokenfold_select_runtime.provision.inspect_bundle',return_value={'revision':'a'*64}),patch('subprocess.run',side_effect=run),patch('subprocess.check_output',side_effect=legacy_bootstrap):
                original=dict(os.environ);provision(root,'b'*64,'cpu',output,'human',root);self.assertEqual(dict(os.environ),original)
            self.assertEqual(len(calls),4)
            self.assertIn('ensurepip',calls[0][0])
            for command,kwargs in calls:
                self.assertIn('-I',command)
                self.assertGreater(kwargs.get('timeout',0),0)
                self.assertTrue(__import__('math').isfinite(kwargs['timeout']))
                self.assertTrue(Path(command[0]).is_absolute())
                self.assertTrue(kwargs.get('check',False))
                for key in ('OPENAI_API_KEY','HTTPS_PROXY','HTTP_PROXY','PYTHONPATH'):
                    self.assertNotIn(key,kwargs['env'])
    def test_bootstrap_failure_keeps_destination_unapproved_and_reserved(self):
        import subprocess
        scratch=Path(__file__).resolve().parents[3]/'.superpowers/sdd/sdk-updates/runtime-scratch'
        scratch.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='provision-fix-',dir=scratch) as directory:
            root=Path(directory);output=root/'approval.json';calls=[]
            def fail(command,**kwargs):
                calls.append(command)
                if 'ensurepip' in command:raise subprocess.TimeoutExpired(command,kwargs.get('timeout',1))
                self.fail('a dependency command ran before explicit ensurepip')
            with patch('tokenfold_select_runtime.provision.inspect_bundle',return_value={'revision':'a'*64}),patch('subprocess.run',side_effect=fail),patch('subprocess.check_output',side_effect=fail):
                with self.assertRaises(subprocess.TimeoutExpired):provision(root,'b'*64,'cpu',output,'human',root)
                self.assertFalse(output.exists());self.assertFalse(output.with_name(output.name+'.pending').exists());self.assertTrue(output.with_name(output.name+'.venv').is_dir())
                with self.assertRaises(FileExistsError):provision(root,'b'*64,'cpu',output,'human',root)
                self.assertEqual(len(calls),1)



class ProvisionExecutableIdentityTests(unittest.TestCase):
    def test_symlinked_venv_approval_preserves_lexical_executable_identity(self):
        import os
        from types import SimpleNamespace
        scratch=Path(__file__).resolve().parents[3]/'.superpowers/sdd/sdk-updates/runtime-scratch'
        scratch.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='provision-fix-',dir=scratch) as directory:
            root=Path(directory);output=root/'approval.json'
            expected=dict(__import__('tokenfold_select_runtime.ettin',fromlist=['ENVIRONMENT']).ENVIRONMENT);expected['torch']='2.11.0+cpu'
            def create(environment):
                executable=environment/('Scripts/python.exe' if os.name=='nt' else 'bin/python')
                executable.parent.mkdir()
                try:executable.symlink_to(Path(sys.executable).resolve())
                except OSError as error:self.skipTest('owned executable symlink unavailable: '+str(error.winerror if os.name=='nt' else error.errno))
            with patch('tokenfold_select_runtime.provision.inspect_bundle',return_value={'revision':'a'*64}),patch('venv.EnvBuilder.create',side_effect=create),patch('subprocess.run',return_value=SimpleNamespace(stdout=json.dumps(expected))):
                config=provision(root,'b'*64,'cpu',output,'human',root)
                self.assertEqual(config['python_executable'],str(output.with_name(output.name+'.venv')/('Scripts/python.exe' if os.name=='nt' else 'bin/python')))
                self.assertNotEqual(config['python_executable'],str(Path(sys.executable).resolve()))

if __name__ == "__main__": unittest.main()
