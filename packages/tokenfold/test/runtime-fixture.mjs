import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';

const hash = bytes => createHash('sha256').update(bytes).digest('hex');
export function runtimeFixture(t) {
  const python = process.env.TOKENFOLD_TEST_PYTHON;
  if (!python) { t.skip('TOKENFOLD_TEST_PYTHON is required only for fake-worker tests'); return; }
  const root = mkdtempSync(join(tmpdir(),'tokenfold-node-runtime-'));
  const source = join(root,'src','tokenfold_select_runtime'); mkdirSync(source,{recursive:true});
  const names = ['__init__.py','__main__.py','ettin.py','integrity.py','protocol.py','provision.py','worker.py'];
  const fake = Buffer.from(readFileSync(new URL('../../../crates/tokenfold-adapters/tests/resident_fake.py',import.meta.url),'utf8').replace("query=body['query']", "query=body['query']; (root/'last-score.json').write_text(json.dumps(body))"));
  const chunks=[];
  for(const name of names) {
    const bytes = name === '__main__.py' ? fake : Buffer.from('# fixture\n');
    writeFileSync(join(source,name),bytes); chunks.push(Buffer.from(name+'\0'),bytes,Buffer.from('\0'));
  }
  const manifest = Buffer.from(JSON.stringify({revision:'a'.repeat(64)}));
  writeFileSync(join(root,'ettin-manifest.json'),manifest); writeFileSync(join(root,'mode'),'normal');
  const approval = {python_executable:resolve(python),python_sha256:hash(readFileSync(python)),runtime_source:source,runtime_source_sha256:hash(Buffer.concat(chunks)),runtime_version:'0.6.0',bundle:root,manifest_sha256:hash(manifest),revision:'a'.repeat(64),device:'cpu',precision:'fp32',approved_by:'test',scratch_root:root,startup_timeout_ms:3000,inference_timeout_ms:5000,cleanup_timeout_ms:100};
  const config = join(root,'approval.json');writeFileSync(config,JSON.stringify(approval));
  t.after(()=>{
    if(existsSync(join(root,'pids'))) for(const line of readFileSync(join(root,'pids'),'utf8').trim().split('\n')) {
      const pid = Number(line.split(':')[0]);
      assert.throws(()=>process.kill(pid,0), e=>e.code==='ESRCH',`owned worker ${pid} must be reaped`);
    }
    rmSync(root,{recursive:true,force:true});
  });
  return {root,select:{config,experimental:true}};
}
export async function waitFor(predicate, message='condition timed out') {
  const end = Date.now()+10000;
  while(!predicate()) { assert.ok(Date.now()<end,message);await new Promise(resolve=>setTimeout(resolve,2)); }
}
export const context = (query='query') => ({kind:'context',query,groups:[{id:'a',content:'alpha '.repeat(40)},{id:'b',content:'beta '.repeat(40)}]});
