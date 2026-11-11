import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync, renameSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { pathToFileURL } from 'node:url';

const root=resolve(import.meta.dirname,'../../..');
const facade=join(root,'packages/tokenfold');
const version=JSON.parse(readFileSync(join(facade,'package.json'))).version;
test('five native manifests are synchronized and install never compiles',()=>{
  const pkg=JSON.parse(readFileSync(join(facade,'package.json')));
  for(const platform of ['darwin-x64','darwin-arm64','linux-x64','linux-arm64','win32-x64']){
    const m=JSON.parse(readFileSync(join(root,'crates/tokenfold-node/npm',platform,'package.json')));
    assert.equal(m.name,`@tokenfold/native-${platform}`);assert.equal(m.version,version);
    assert.equal(pkg.optionalDependencies[m.name],version);assert.equal(m.main,'tokenfold.node');
    assert.equal(m.repository.url,pkg.repository.url);
    assert.equal(m.repository.directory,`crates/tokenfold-node/npm/${platform}`);
    assert.equal(m.scripts?.install,undefined);assert.equal(m.scripts?.postinstall,undefined);
  }
  assert.equal(pkg.scripts.install,undefined);assert.equal(pkg.scripts.postinstall,undefined);
});
test('fresh packed install outside checkout: native Core, fake worker, missing addon and installed types', {timeout:120000},()=>{
  const temp=mkdtempSync(join(tmpdir(),'tokenfold-native-install-'));
  try {
    const npm=process.env.npm_execpath ?? join(dirname(process.execPath),'node_modules/npm/bin/npm-cli.js');
    const call=(args,cwd=temp)=>execFileSync(process.execPath,[npm,...args],{cwd,encoding:'utf8',stdio:['ignore','pipe','pipe']});
    const pack=(dir)=>JSON.parse(call(['pack','--json','--ignore-scripts','--pack-destination',temp],dir))[0];
    const platform=`${process.platform}-${process.arch}`;
    const stage=join(temp,'native');mkdirSync(stage);
    copyFileSync(join(root,'crates/tokenfold-node/npm',platform,'package.json'),join(stage,'package.json'));
    copyFileSync(join(facade,'native/tokenfold.node'),join(stage,'tokenfold.node'));
    copyFileSync(join(root,'LICENSE'),join(stage,'LICENSE'));
    const nativePack=pack(stage),sdkPack=pack(facade);
    assert.ok(nativePack.files.some(f=>f.path==='LICENSE'));
    assert.equal(sdkPack.files.some(f=>f.path.endsWith('.node')),false);
    assert.ok(sdkPack.files.some(f=>f.path==='dist/index.d.ts'));
    const install=join(temp,'installed');mkdirSync(install);
    writeFileSync(join(install,'package.json'),JSON.stringify({private:true,type:'module'}));
    call(['install','--offline','--no-audit','--no-fund','--omit=optional',join(temp,nativePack.filename),join(temp,sdkPack.filename)],install);
    const script=join(install,'smoke.mjs');
    const runtimeURL=pathToFileURL(join(facade,'test/runtime-fixture.mjs')).href;
    writeFileSync(script,`
      import assert from 'node:assert/strict';
      import {fit,Tokenfold,Receipt} from 'tokenfold';
      import {binaryPath,run} from 'tokenfold/cli';
      import {runtimeFixture,context} from ${JSON.stringify(runtimeURL)};
      assert.equal(typeof binaryPath,'function');assert.equal(typeof run,'function');
      const result=await fit('fresh native install',{targetTokens:100});
      assert.equal(result.requireMet(),'fresh native install');
      assert.deepEqual(Receipt.fromJSON(result.receipt.toJSON()).toJSON(),result.receipt.toJSON());
      const cleanup=[];const f=runtimeFixture({skip(){},after(cb){cleanup.push(cb);}});
      if(f){const client=new Tokenfold({select:f.select});try{assert.equal((await client.fit(context(),{targetTokens:8,allowSelection:true})).selectionApplied,true);}finally{await client.close();for(const cb of cleanup)await cb();}}
      console.log(JSON.stringify({version:${JSON.stringify(version)},node:process.version,core:true,fakeWorker:!!f}));
    `);
    const cleanEnv={...process.env,PATH:'',TOKENFOLD_BINARY_PATH:join(temp,'must-not-run.exe')};
    const output=execFileSync(process.execPath,[script],{cwd:install,env:cleanEnv,encoding:'utf8'});
    assert.equal(JSON.parse(output).core,true);
    copyFileSync(join(root,'examples/sdk_fit.mjs'),join(install,'example.mjs'));
    execFileSync(process.execPath,['example.mjs'],{cwd:install,env:cleanEnv,stdio:'pipe'});
    for (const name of ['quickstart.mjs','openai_payload.json','api_response.json']) {
      copyFileSync(join(root,'examples',name),join(install,name));
    }
    const quickstart=execFileSync(process.execPath,['quickstart.mjs'],{cwd:install,env:{...cleanEnv,TOKENFOLD_BINARY_PATH:process.env.TOKENFOLD_TEST_BINARY},encoding:'utf8'});
    assert.match(quickstart,/== explicit TOON ==/);
    // Reuse the real lifecycle/parity suite against installed JS and the packed addon.
    for (const name of ['fit.test.mjs','native.test.mjs','parity.test.mjs']) {
      const source=readFileSync(join(facade,'test',name),'utf8')
        .replaceAll("'../dist/", "'./node_modules/tokenfold/dist/")
        .replaceAll("'./runtime-fixture.mjs'",JSON.stringify(runtimeURL))
        .replaceAll('../../../tests/fixtures/sdk-fit/',pathToFileURL(join(root,'tests/fixtures/sdk-fit')+'/').href);
      writeFileSync(join(install,name),source);
    }
    execFileSync(process.execPath,['--test','fit.test.mjs','native.test.mjs','parity.test.mjs'],{cwd:install,env:cleanEnv,stdio:'pipe'});
    copyFileSync(join(facade,'test/fit-types.ts'),join(install,'fit-types.ts'));
    execFileSync(process.execPath,[join(facade,'node_modules/typescript/bin/tsc'),'--noEmit','--strict','--target','ES2022','--lib','ES2024','--module','NodeNext','--moduleResolution','NodeNext','--typeRoots',join(facade,'node_modules/@types'),'fit-types.ts'],{cwd:install,stdio:'pipe'});
    const addon=join(install,'node_modules/@tokenfold',`native-${platform}`,'tokenfold.node');
    renameSync(addon,addon+'.held');
    const missing=execFileSync(process.execPath,['--input-type=module','-e',`import assert from 'node:assert/strict';import {fit,InstallationError} from 'tokenfold';await assert.rejects(fit('text',{targetTokens:10}),InstallationError);console.log('missing addon rejected')`],{cwd:install,env:cleanEnv,encoding:'utf8'});
    assert.match(missing,/missing addon rejected/);
    renameSync(addon+'.held',addon);
    const installedManifest=join(install,'node_modules/tokenfold/package.json');
    const mismatch=JSON.parse(readFileSync(installedManifest));mismatch.version='0.0.0';writeFileSync(installedManifest,JSON.stringify(mismatch));
    execFileSync(process.execPath,['--input-type=module','-e',`import assert from 'node:assert/strict';import {fit,InstallationError} from 'tokenfold';await assert.rejects(fit('text',{targetTokens:10}),InstallationError);`],{cwd:install,env:cleanEnv,encoding:'utf8'});
    console.log(`installed artifact receipts ${JSON.stringify({sdk:sdkPack.integrity,native:nativePack.integrity,...JSON.parse(output)})}`);
  } finally { rmSync(temp,{recursive:true,force:true}); }
});
