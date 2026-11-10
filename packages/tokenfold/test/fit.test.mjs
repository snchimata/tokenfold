import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import * as tf from '../dist/index.js';

const fixture = name => JSON.parse(readFileSync(new URL(`../../../tests/fixtures/sdk-fit/${name}.json`, import.meta.url)));
test('native fitting API replaces the CLI primary entry', () => {
  assert.equal(typeof tf.fit, 'function');
  assert.equal(typeof tf.Tokenfold, 'function');
  assert.equal(tf.compress, undefined);
});

test('invalid inputs never coerce or reach custom counters', async () => {
  let calls = 0;
  const counter = {id:'test:v1', count(){ calls++; return {tokens:1,basis:'exact'}; }};
  for (const targetTokens of [true,-1,1.2,2**53,'20',null]) await assert.rejects(tf.fit('text',{targetTokens,counter}),tf.InvalidInputError);
  const cyclic = []; cyclic.push(cyclic);
  for (const value of [cyclic,NaN,Infinity,2**53,new Date(),new Uint8Array(2),{x:undefined},{x:'\ud800'},[,,]]) await assert.rejects(tf.fit({kind:'json_data',value},{targetTokens:10,counter}),tf.InvalidInputError);
  for (const request of [{kind:'text',text:'private internal envelope'}, {kind:'context',query:'q',groups:[{id:'a',content:{kind:'text',text:'not public'}}]}]) await assert.rejects(tf.fit(request,{targetTokens:10,counter}),tf.InvalidInputError);
  await assert.rejects(tf.fit('Authorization: Bearer '+'x'.repeat(32),{targetTokens:100,counter}),tf.SafetyError);
  assert.equal(calls,0);
});
test('null configuration, malformed metadata and sparse arrays with extra properties reject', async () => {
  for(const options of [{maxPendingScores:null},{scoreWaitTimeoutMs:null},{counter:null}]) assert.throws(()=>new tf.Tokenfold(options),tf.ConfigError);
  const sparse = new Array(1); sparse.extra=1;
  await assert.rejects(tf.fit({kind:'json_data',value:sparse},{targetTokens:10}),tf.InvalidInputError);
  await assert.rejects(tf.fit('text',{targetTokens:10,counter:{id:'bad\ud800',count:()=>({tokens:1,basis:'exact'})}}),tf.AccountingError);
  assert.throws(()=>new tf.Tokenfold({select:{config:'bad\ud800',experimental:true}}),tf.ConfigError);
});
test('custom exceptions, invalid counts and counter identity drift are sanitized', async () => {
  for (const count of [{tokens:true,basis:'exact'},{tokens:-1,basis:'estimated'},{tokens:0,basis:'unavailable'},{tokens:null,basis:'exact'},{tokens:2**53,basis:'exact'}]) await assert.rejects(tf.fit('text',{targetTokens:20,counter:{id:'counter:v1',count:()=>count}}),tf.AccountingError);
  await assert.rejects(tf.fit('text',{targetTokens:20,counter:{id:'counter:v1',count(){throw Error('private source secret');}}}),e=> e instanceof tf.AccountingError && !String(e).includes('private'));
  const counter = {id:'before:v1',count(){this.id='after:v1'; return {tokens:1,basis:'exact'};}};
  await assert.rejects(tf.fit('text',{targetTokens:20,counter}),tf.AccountingError);
});
test('snapshots are owned and provider message groups default optional', async () => {
  const payload = {model:'model',messages:[{role:'system',content:'instruction'},{role:'assistant',content:'old'},{role:'user',content:'active'}],tools:[{name:'lookup'}]};
  const request = {kind:'openai_chat',payload,groups:[{id:'old',indices:[1]}]};
  let calls = 0;
  const promise = tf.fit({...request},{targetTokens:100,counter:{id:'full:v1',count(r){calls++;assert.deepEqual(r.payload.tools,[{name:'lookup'}]);assert.throws(()=>r.payload.messages.pop(),TypeError);return {tokens:12,basis:'exact'};}}});
  payload.messages[1].content = 'caller mutation';
  const r = await promise;
  assert.equal(r.payload.messages[1].content,'old');
  r.payload.messages.length = 0;
  assert.equal(r.payload.messages.length,3);
  assert.deepEqual(r.groupsKept,['__tf_message_0','old','__tf_message_2']);
  assert.equal(calls,1);
  for(const indices of [[0],[2],[1,3],[2,1]]) await assert.rejects(tf.fit({...request,groups:[{id:'bad',indices}]},{targetTokens:100}),tf.InvalidInputError);
});
test('receipt reader validates wire only and defensive round trips', async () => {
  const r = (await tf.fit('text',{targetTokens:20})).receipt;
  const wire = r.toJSON(); wire.target_tokens = 0;
  assert.equal(r.targetTokens,20);
  for(const value of [r,{...r.toJSON(),schema_version:'2.0'},{...r.toJSON(),budgetStatus:'met'},{...r.toJSON(),final_tokens:-1}]) assert.throws(()=>tf.Receipt.fromJSON(value),tf.InvalidInputError);
});
test('closed clients, explicit selection config and already-aborted signals', async () => {
  for (const options of [{maxPendingScores:true},{maxPendingScores:129},{scoreWaitTimeoutMs:0},{select:{config:'none'}}]) assert.throws(()=>new tf.Tokenfold(options),tf.ConfigError);
  const client = new tf.Tokenfold();
  await assert.rejects(client.warmup(),tf.ConfigError);
  const controller = new AbortController(); controller.abort();
  await assert.rejects(client.fit('text',{targetTokens:20,signal:controller.signal}),tf.CancelledError);
  for(const budgetMode of ['unknown',null,false]) await assert.rejects(client.fit('text',{targetTokens:20,budgetMode}),tf.ConfigError);
  for(const allowSelection of [null,1]) await assert.rejects(client.fit('text',{targetTokens:20,allowSelection}),tf.ConfigError);
  await client.close(); await client.close();
  await assert.rejects(client.fit('text',{targetTokens:20}),tf.ClosedError);
  await assert.rejects(client.warmup(),tf.ClosedError);
});
test('malformed structural group elements raise input errors rather than internal failures',async()=>{
  for(const group of [null,1,[],true]) await assert.rejects(tf.fit({kind:'context',query:'q',groups:[group]},{targetTokens:20}),tf.InvalidInputError);
});
test('delayed custom callbacks are concurrent and cancellation waits for their cleanup', async () => {
  let entered = 0, release;
  const gate = new Promise(resolve=>release=resolve);
  const client = new tf.Tokenfold({counter:{id:'concurrent:v1',async count(){entered++; await gate;return {tokens:1,basis:'exact'};}}});
  const cancel = new AbortController();
  const first = client.fit('one',{targetTokens:10,signal:cancel.signal});
  const rejected = assert.rejects(first,tf.CancelledError);
  const second = client.fit('two',{targetTokens:10});
  const deadline=Date.now()+10000;
  while(entered < 2) { assert.ok(Date.now()<deadline,'concurrent callbacks were not admitted');await new Promise(resolve=>setTimeout(resolve,2)); }
  let settled = false; void first.catch(()=>{settled=true;}); cancel.abort();
  await new Promise(resolve=>setTimeout(resolve,10)); assert.equal(settled,false);
  release(); await rejected; assert.equal((await second).requireMet(),'two');
  await client.close();
});
test('shared Core and receipt fixture, async counter and canonical casing', async () => {
  const f = fixture('core');
  const seen = [];
  const counter = { id: f.counter_id, async count(request) {
    seen.push(request.payload);
    await new Promise(resolve => setTimeout(resolve, 1));
    return request.payload === f.original_payload ? f.original_count : f.proposal_count;
  }};
  const r = await tf.fit({...f.request}, {targetTokens:f.target_tokens, counter});
  assert.equal(r.requireMet(), f.proposal_payload);
  assert.equal(r.tokensSaved, 8);
  assert.equal(r.receipt.accounting.counterId, f.counter_id);
  const wire = r.receipt.toJSON();
  delete wire.timings_ms;
  const expected = fixture('receipt'); delete expected.timings_ms;
  assert.deepEqual(wire, expected);
  assert.deepEqual(tf.Receipt.fromJSON(r.receipt.toJSON()).toJSON(), r.receipt.toJSON());
  assert.equal(r.receipt.schema_version, undefined);
  assert.deepEqual(seen, [f.original_payload, f.proposal_payload]);
  assert.throws(() => { r.receipt.accounting.counterId = 'changed'; }, TypeError);
});
test('structural inputs, nested groups and safe numeric canonicalization', async () => {
  const f = fixture('core').context;
  const r = await tf.fit({kind:'context', query:f.query, groups:f.groups.map(g => ({...g, content:typeof g.content === 'string' ? g.content : {kind:'json_data',value:g.content}}))}, {targetTokens:100});
  assert.equal(r.payload, f.rendered);
  assert.deepEqual(r.groupsKept, ['optional','required']);
  const numbers = fixture('numbers');
  for (const value of numbers.equivalent_inputs) assert.equal((await tf.fit({kind:'json_data',value}, {targetTokens:100})).payload, numbers.canonical_json);
  assert.equal((await tf.fit(' Unicode 🐎\n', {targetTokens:100})).payload, ' Unicode 🐎\n');
});
test('native built-ins and provider estimates keep exact and estimated evidence separate', async () => {
  const f = fixture('provider_estimates');
  const counter = tf.ProviderCounter.openai({model:f.model});
  counter.count = () => { throw Error('host callback must not run'); };
  for (const c of f.cases) {
    const r = await tf.fit({kind:'openai_chat',payload:c.payload}, {targetTokens:100,counter,budgetMode:'best_effort'});
    assert.deepEqual(r.payload,c.payload);
    assert.equal(r.finalTokens,c.count.tokens);
    assert.equal(r.receipt.accounting.finalBasis,c.count.basis);
    assert.throws(()=>r.requireMet(),tf.BudgetIndeterminateError);
    if(c.count.basis==='estimated') assert.deepEqual(r.requireFit(),c.payload);
    else assert.throws(()=>r.requireFit(),tf.BudgetIndeterminateError);
  }
  const encoding = new tf.EncodingCounter();
  encoding.count = () => { throw Error('host callback must not run'); };
  assert.equal((await tf.fit('native', {targetTokens:100,counter:encoding})).requireMet(),'native');
});
test('standalone built-in counters run native counts and validate their configuration',async()=>{
  const encoding=new tf.EncodingCounter('cl100k_base');
  assert.deepEqual(await encoding.count({format:'text',payload:'Hello',scope:'rendered_text'}),{tokens:1,basis:'exact'});
  assert.deepEqual(await encoding.count({format:'openai_chat',payload:{messages:[]},scope:'provider_request'}),{tokens:null,basis:'unavailable'});
  const fixtureData=fixture('provider_estimates'),provider=tf.ProviderCounter.openai({model:fixtureData.model});
  for(const c of fixtureData.cases) assert.deepEqual(await provider.count({format:'openai_chat',scope:'provider_request',payload:c.payload}),c.count);
  assert.throws(()=>new tf.EncodingCounter('unknown'),tf.ConfigError);
  assert.throws(()=>tf.ProviderCounter.openai({model:'unknown'}),tf.ConfigError);
});
