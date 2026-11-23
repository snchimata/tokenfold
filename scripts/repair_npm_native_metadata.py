"""Add only repository metadata to the unpublished 0.6.0 native tarballs."""
import copy
import gzip
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile

PLATFORMS = ('darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64')


def repack(source, manifest, output):
    if source.resolve() == output.resolve():
        raise ValueError('original tarball must not be overwritten')
    with tarfile.open(source) as archive:
        members = archive.getmembers()
        names = {'package/LICENSE', 'package/tokenfold.node', 'package/package.json'}
        if len(members) != 3 or {m.name for m in members} != names or not all(m.isfile() for m in members):
            raise ValueError('unexpected native tarball contents')
        files = {m.name: archive.extractfile(m).read() for m in members}
    original = json.loads(files['package/package.json'])
    expected = {k: v for k, v in manifest.items() if k != 'repository'}
    if original != expected or original.get('version') != '0.6.0':
        raise ValueError('native metadata differs beyond the missing repository')
    files['package/package.json'] = (json.dumps(manifest, indent=2) + '\n').encode()
    with output.open('wb') as raw, gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as archive:
            for member in sorted(members, key=lambda m: m.name):
                info = copy.copy(member)
                info.mtime, info.pax_headers = 0, {}
                info.size = len(files[info.name])
                archive.addfile(info, io.BytesIO(files[info.name]))
    with tarfile.open(output) as archive:
        for name, data in files.items():
            if archive.extractfile(name).read() != data:
                raise ValueError('repacked native bytes changed')
    return hashlib.sha256(files['package/tokenfold.node']).hexdigest()


def main():
    source, output = map(Path, sys.argv[1:])
    output.mkdir(parents=True, exist_ok=True)
    root = Path(__file__).resolve().parents[1]
    receipts = []
    for platform in PLATFORMS:
        filename = f'tokenfold-native-{platform}-0.6.0.tgz'
        manifest = json.loads((root / f'crates/tokenfold-node/npm/{platform}/package.json').read_text())
        expected_repository = {'type': 'git', 'url': 'git+https://github.com/snchimata/tokenfold.git',
                               'directory': f'crates/tokenfold-node/npm/{platform}'}
        if manifest.get('repository') != expected_repository:
            raise ValueError('repository does not match the publishing source')
        node_hash = repack(source / filename, manifest, output / filename)
        receipts.append({'file': filename, 'original_sha256': hashlib.sha256((source / filename).read_bytes()).hexdigest(),
                         'sha256': hashlib.sha256((output / filename).read_bytes()).hexdigest(),
                         'unchanged_addon_sha256': node_hash})
    (output / 'SHA256SUMS').write_text(''.join(f"{r['sha256']}  {r['file']}\n" for r in receipts), encoding='utf8')
    (output / 'metadata-repair.json').write_text(json.dumps(receipts, indent=2) + '\n', encoding='utf8')
    print(json.dumps(receipts, indent=2))


if __name__ == '__main__':
    main()
