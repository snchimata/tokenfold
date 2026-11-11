import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

const workflow = new URL('../.github/workflows/bootstrap-npm-native.yml', import.meta.url);
const source = (await readFile(workflow, 'utf8')).replaceAll('\r\n', '\n');

test('native bootstrap is manual and reuses the original tested publication artifact', () => {
  assert.match(source, /on: workflow_dispatch/);
  assert.doesNotMatch(source, /pull_request:|push:/);
  assert.match(source, /artifact-ids: 13827437119/);
  assert.match(source, /run-id: 42955400102/);
  assert.match(source, /merge-multiple: true/);
  assert.match(source, /id-token: write/);
  assert.match(source, /actions: read/);
  assert.match(source, /github-token: \$\{\{ github.token \}\}/);
});

test('native bootstrap verifies all five immutable tarballs before staging', () => {
  const rows = [...source.matchAll(/^\s+([a-f0-9]{64})  tokenfold-native-([\w-]+)-0\.6\.0\.tgz$/gm)];
  assert.deepEqual(rows.map(row => row[2]).sort(),
    ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64']);
  assert.equal(new Set(rows.map(row => row[1])).size, 5);
  assert.ok(source.indexOf('sha256sum -c') < source.indexOf('npm stage publish'));
  assert.ok(source.indexOf('sha256sum -c') < source.indexOf('repair_npm_native_metadata.py'));
  assert.ok(source.indexOf('repair_npm_native_metadata.py') < source.indexOf('npm stage publish'));
  assert.match(source, /name: native-bootstrap-tarballs/);
  assert.match(source, /set -euo pipefail/);
});

test('native bootstrap preserves provenance and requires separate human approval', () => {
  assert.match(source, /npm stage publish .*--access public --provenance --ignore-scripts --json/);
  assert.match(source, /npm view "\$package@0\.6\.0" version/);
  assert.doesNotMatch(source, /npm publish|npm stage approve|--otp|--provenance=false|npm access/);
  assert.match(source, /NODE_AUTH_TOKEN: \$\{\{ secrets.NPM_TOKEN \}\}/);
  assert.match(source, /Approve.*2FA/);
});

test('bootstrap shell stages only verified bytes and any checksum failure blocks every upload', async () => {
  const blocks = [...source.matchAll(/^        run: \|\n((?:^          .*\n|^\n)*)/gm)]
    .map(match => match[1].replace(/^ {10}/gm, ''));
  let shell = blocks[0] + '\ncd ..\n' + blocks.at(-1);
  const directory = await mkdtemp(join(tmpdir(), 'tokenfold-bootstrap-test-'));
  const bash = process.platform === 'win32'
    ? resolve(execFileSync('git', ['--exec-path'], {encoding: 'utf8'}).trim(), '../../../bin/bash.exe')
    : 'bash';
  try {
    await mkdir(join(directory, 'tarballs'));
    await mkdir(join(directory, 'corrected'));
    await mkdir(join(directory, 'bin'));
    const files = [...source.matchAll(/([a-f0-9]{64})  (tokenfold-native-[\w.-]+\.tgz)/g)];
    let checksums = '';
    for (const [row, , name] of files) {
      const bytes = Buffer.from(`fixture:${name}`);
      await writeFile(join(directory, 'tarballs', name), bytes);
      await writeFile(join(directory, 'corrected', name), bytes);
      const checksum = `${createHash('sha256').update(bytes).digest('hex')}  ${name}`;
      shell = shell.replace(row, checksum);
      checksums += checksum + '\n';
    }
    await writeFile(join(directory, 'corrected/SHA256SUMS'), checksums);
    const npm = join(directory, 'bin/npm');
    await writeFile(npm, '#!/usr/bin/env bash\nset -eu\nif [[ "$1" == view ]]; then exit 1; fi\nprintf "%s\\n" "$*" >> "$NPM_CALLS"\n[[ "$1 $2" == "stage publish" ]]\necho \'{"stage":"fixture"}\'\n');
    await chmod(npm, 0o755);
    const run = () => execFileSync(bash, ['-c',
      'export PATH="$PWD/bin:$PATH"; export NPM_CALLS="$PWD/calls"; export GITHUB_STEP_SUMMARY="$PWD/summary";\n' + shell],
      {cwd: directory, env: {...process.env, NODE_AUTH_TOKEN: 'fixture-not-a-credential'}, stdio: 'pipe'});
    run();
    const calls = (await readFile(join(directory, 'calls'), 'utf8')).trim().split('\n');
    assert.equal(calls.length, 5);
    assert.ok(calls.every(call => /^stage publish .* --access public --provenance --ignore-scripts --json$/.test(call)));
    await writeFile(join(directory, 'calls'), '');
    await writeFile(join(directory, 'tarballs', files.at(-1)[2]), 'tampered');
    assert.throws(run);
    assert.equal(await readFile(join(directory, 'calls'), 'utf8'), '');
    const name = files.at(-1)[2];
    await writeFile(join(directory, 'tarballs', name), Buffer.from(`fixture:${name}`));
    await writeFile(join(directory, 'corrected', name), 'tampered');
    assert.throws(run);
    assert.equal(await readFile(join(directory, 'calls'), 'utf8'), '');
  } finally {
    assert.ok(directory.startsWith(join(tmpdir(), 'tokenfold-bootstrap-test-')));
    await rm(directory, {recursive: true, force: true});
  }
});

test('metadata-only repair rejects unexpected manifests and archive members', () => {
  execFileSync(process.env.TOKENFOLD_TEST_PYTHON ?? 'python',
    [join(import.meta.dirname, 'test_npm_native_metadata.py')], {stdio: 'pipe'});
});
