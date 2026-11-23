import copy
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from repair_npm_native_metadata import repack


class NativeMetadataTest(unittest.TestCase):
    def test_metadata_only_repack_is_deterministic_and_rejects_unexpected_inputs(self):
        manifest = json.loads((Path(__file__).resolve().parents[1] /
                               'crates/tokenfold-node/npm/win32-x64/package.json').read_text())
        self.assertIn('repository', manifest)
        original = copy.deepcopy(manifest)
        del original['repository']
        files = {'package/LICENSE': b'license bytes', 'package/tokenfold.node': b'compiled addon',
                 'package/package.json': json.dumps(original).encode()}
        with tempfile.TemporaryDirectory(prefix='tokenfold-native-metadata-') as tmp:
            root = Path(tmp)
            source = root / 'original.tgz'
            def fixture(members):
                with tarfile.open(source, 'w:gz') as tar:
                    for name, data in members.items():
                        info = tarfile.TarInfo(name)
                        info.size = len(data)
                        info.mode = 0o644
                        tar.addfile(info, io.BytesIO(data))
            fixture(files)
            before = source.read_bytes()
            outputs = [root / 'first.tgz', root / 'second.tgz']
            for output in outputs:
                repack(source, manifest, output)
                with tarfile.open(output) as tar:
                    self.assertEqual({m.name for m in tar}, set(files))
                    self.assertEqual(json.load(tar.extractfile('package/package.json')), manifest)
                    for name in ('package/LICENSE', 'package/tokenfold.node'):
                        self.assertEqual(tar.extractfile(name).read(), files[name])
            self.assertEqual(outputs[0].read_bytes(), outputs[1].read_bytes())
            self.assertEqual(source.read_bytes(), before)
            with self.assertRaises(ValueError):
                repack(source, manifest, source)
            invalid = copy.deepcopy(manifest)
            invalid['version'] = '9.9.9'
            with self.assertRaises(ValueError):
                repack(source, invalid, root / 'invalid.tgz')
            fixture({**files, '../escape': b'not allowed'})
            with self.assertRaises(ValueError):
                repack(source, manifest, root / 'invalid.tgz')
            fixture({name: data for name, data in files.items() if not name.endswith('.node')})
            with self.assertRaises(ValueError):
                repack(source, manifest, root / 'invalid.tgz')


if __name__ == '__main__':
    unittest.main()
