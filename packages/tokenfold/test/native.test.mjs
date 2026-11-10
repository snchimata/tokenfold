import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import * as tf from '../dist/index.js';
import { native } from '../dist/native.js';
import { runtimeFixture, waitFor, context } from './runtime-fixture.mjs';

test('started native Core work cancels through its token while event loop remains responsive', async () => {
  const n = native(), client = new n.NativeClient(undefined,32,5000), cancel = new n.NativeCancellation();
  let ticks = 0;
  const heartbeat = setInterval(()=>ticks++,1);
  const running = client.begin(JSON.stringify({kind:'text',text:'words and tokens '.repeat(100000)}),JSON.stringify({target:1000000,mode:'strict',allow_selection:false,counter_kind:'encoding',counter_value:'o200k_base'}),cancel);
  const rejected = assert.rejects(running,e=>e.message==='CancelledError');
  await waitFor(()=>cancel.started,'native session was not started');
  cancel.cancel();
  await rejected; clearInterval(heartbeat);
  assert.ok(ticks>0,'native compute must not block the event loop');
  await client.close();
});
test('actual session checkpoints prevent custom callbacks after abort or close during counter ID read', async () => {
  for(const mode of ['abort','close']) {
    let reads=0, calls=0, closing;
    const abort = new AbortController();
    const counter = {get id(){ if(++reads===2) { if(mode==='abort') abort.abort();else closing=client.close(); }return 'checkpoint:v1';},count(){calls++;return {tokens:1,basis:'exact'};}};
    const client = new tf.Tokenfold({counter});
    await assert.rejects(client.fit('text',{targetTokens:10,signal:abort.signal}),tf.CancelledError);
    assert.equal(calls,0); await closing; await client.close();
  }
});
test('paused native Count handoff never starts a callback after cancellation or close', async () => {
  for(const mode of ['abort','close']) {
    const proto=native().NativeClient.prototype,begin=proto.begin;
    let produced=false,release,calls=0;
    const gate=new Promise(resolve=>release=resolve);
    proto.begin=async function(...args){const session=await begin.apply(this,args);assert.equal(JSON.parse(session.action).action,'count');produced=true;await gate;return session;};
    const abort=new AbortController();const client=new tf.Tokenfold({counter:{id:'handoff:v1',count(){calls++;return {tokens:1,basis:'exact'};}}});
    try {
      const running=client.fit('text',{targetTokens:10,signal:abort.signal});
      const rejected=assert.rejects(running,tf.CancelledError);
      await waitFor(()=>produced);
      const closing=mode==='close'?client.close():undefined;if(mode==='abort')abort.abort();
      release();await rejected;await closing;assert.equal(calls,0);
    } finally {release();proto.begin=begin;await client.close();}
  }
});
test('native boundary validates protocol replies and receipt duplicates', async () => {
  const n=native(), c=new n.NativeClient(undefined,32,5000);
  const f=await c.begin('{"kind":"text","text":"hello"}',JSON.stringify({target:10,mode:'strict',allow_selection:false,counter_kind:'custom',counter_value:'boundary:v1'}),new n.NativeCancellation());
  assert.equal(JSON.parse(f.action).action,'count');
  await assert.rejects(f.step('{"tokens":true,"basis":"exact"}','boundary:v1'),e=>e.message==='AccountingError');
  f.finish(); await c.close();
  const wire=JSON.stringify((await tf.fit('text',{targetTokens:10})).receipt);
  assert.throws(()=>n.receiptJson(wire.replace('"kind":','"kind":"tokenfold.fit","kind":')),e=>e.message==='InvalidInputError');
});
test('fake worker selection, cache, consent, estimates, failures and warmup reuse', async t => {
  const fixture=runtimeFixture(t);if(!fixture)return;
  const {root,select}=fixture;let calls=[];
  const counter={id:'custom:length:v1',count(r){calls.push(r.payload);return {tokens:r.payload.length,basis:'exact'};}};
  const client=new tf.Tokenfold({counter,select});t.after(()=>client.close());
  assert.equal((await client.fit('fits',{targetTokens:20})).budgetStatus,'met');
  assert.equal((await client.fit(context(),{targetTokens:20})).reason,'selection_disabled');
  assert.equal(existsSync(join(root,'pids')),false);
  calls=[];
  const r=await client.fit(context(),{targetTokens:20,allowSelection:true});
  assert.equal(r.requireMet(),'query');assert.deepEqual(r.groupsDropped,['a','b']);assert.equal(r.selectionApplied,true);
  assert.equal(r.receipt.scorer.qualification,'unqualified');assert.equal(typeof r.receipt.scorer.manifestHash,'string');
  assert.equal(calls.length,2,'required-only candidate must be cached');
  await Promise.all([client.warmup(),client.warmup()]);await client.warmup();
  assert.equal(readFileSync(join(root,'pids'),'utf8').trim().split('\n').length,1);
  const fail=await client.fit(context('nan'),{targetTokens:20,allowSelection:true});
  assert.equal(fail.reason,'scorer_response_invalid');assert.equal(fail.receipt.scorerSucceeded,false);
  assert.deepEqual(fail.groupsKept,['a','b']);assert.throws(()=>fail.requireMet(),tf.BudgetUnmetError);
  await client.close();
  const estimated=new tf.Tokenfold({select,counter:{id:'estimate:v1',count:r=>({tokens:r.payload.length,basis:'estimated'})}});t.after(()=>estimated.close());
  assert.equal((await estimated.fit(context(),{targetTokens:20,allowSelection:true})).reason,'unknown_accounting');
  const best=await estimated.fit(context(),{targetTokens:20,allowSelection:true,budgetMode:'best_effort'});
  assert.equal(best.requireFit(),'query');assert.equal(best.budgetStatus,'indeterminate');assert.equal(best.reason,'estimated_fit');
  assert.throws(()=>best.requireMet(),tf.BudgetIndeterminateError);await estimated.close();
});
test('score waits do not occupy libuv; waiting abort does not kill active child; queue saturation only', async t => {
  const fixture=runtimeFixture(t);if(!fixture)return;
  const {root,select}=fixture;
  const client=new tf.Tokenfold({select,maxPendingScores:1});t.after(()=>client.close());
  const activeCancel=new AbortController(),waitCancel=new AbortController();
  const active=client.fit(context('hold'),{targetTokens:8,allowSelection:true,signal:activeCancel.signal});
  const activeRejected=assert.rejects(active,e=>e instanceof tf.CancelledError);
  await waitFor(()=>existsSync(join(root,'active')));
  // Native begin resolves only once the built-in-counter request has obtained its scorer ticket.
  const prototype=native().NativeClient.prototype, begin=prototype.begin;
  let admitted=false;
  prototype.begin=async function(...args){const result=await begin.apply(this,args);if(JSON.parse(result.action).action==='score')admitted=true;return result;};
  let waiting;
  try { waiting=client.fit(context(),{targetTokens:8,allowSelection:true,signal:waitCancel.signal});await waitFor(()=>admitted); }
  finally {prototype.begin=begin;}
  const waitingRejected=assert.rejects(waiting,tf.CancelledError);
  await assert.rejects(client.fit(context(),{targetTokens:8,allowSelection:true}),tf.BusyError);
  const core=await Promise.race([client.fit('Core fits',{targetTokens:20}),new Promise((_,reject)=>{const timer=setTimeout(()=>reject(Error('Core blocked by scorer wait')),2000);timer.unref();})]);
  assert.equal(core.budgetStatus,'met');
  waitCancel.abort();await waitingRejected;
  assert.equal(readFileSync(join(root,'active'),'utf8'),'hold');
  assert.equal(readFileSync(join(root,'pids'),'utf8').trim().split('\n').length,1);
  activeCancel.abort();await activeRejected;
  assert.equal((await client.fit(context(),{targetTokens:8,allowSelection:true})).selectionApplied,true);
  await client.close();
});
test('queue timeout and close wake waiters and reap the owned child', async t => {
  const fixture=runtimeFixture(t);if(!fixture)return;
  const {root,select}=fixture;
  const client=new tf.Tokenfold({select,scoreWaitTimeoutMs:30});t.after(()=>client.close());
  const active=client.fit(context('hold'),{targetTokens:8,allowSelection:true});
  const rejected=assert.rejects(active,e=>e instanceof tf.CancelledError||e instanceof tf.ClosedError);
  await waitFor(()=>existsSync(join(root,'active')));
  const timeout=await client.fit(context(),{targetTokens:8,allowSelection:true});
  assert.equal(timeout.reason,'scorer_queue_timeout');assert.equal(timeout.receipt.scorerAttempted,false);
  await client.close();await rejected;
});
test('shared provider fixtures retain order, unknown fields and original scorer projections', async t => {
  const f=runtimeFixture(t);if(!f)return;
  const client=new tf.Tokenfold({select:f.select,counter:{id:'fixture:whole-provider:v1',count:r=>({tokens:r.payload.messages.length===4?200:1,basis:'exact'})}});t.after(()=>client.close());
  for(const [name,kind] of [['openai','openai_chat'],['anthropic','anthropic_messages']]) {
    const fixture=JSON.parse(readFileSync(new URL(`../../../tests/fixtures/sdk-fit/${name}.json`,import.meta.url)));
    const r=await client.fit({kind,payload:fixture.payload,query:'query',groups:[{id:'history',indices:[1,2]}]},{targetTokens:100,allowSelection:true});
    assert.equal(JSON.stringify(r.payload),JSON.stringify(fixture.required_payload));
    assert.deepEqual(r.groupsDropped,['history']);assert.deepEqual(r.groupsKept,fixture.source_ids.filter(id=>id!=='history'));
    const score=JSON.parse(readFileSync(join(f.root,'last-score.json'),'utf8'));
    assert.deepEqual(score.groups.map(g=>g.id),fixture.source_ids);assert.deepEqual(score.groups.map(g=>g.text),fixture.scoring_texts);
  }
  await client.close();
});
test('shared selection decisions include nonadditive recount and conservative required preflight', async t => {
  const f=runtimeFixture(t);if(!f)return;
  const fixture=JSON.parse(readFileSync(new URL('../../../tests/fixtures/sdk-fit/selection.json',import.meta.url)));
  for(const c of fixture.cases) {
    const counter={id:'fixture:selection:v1',count(r){const n=r.payload.split('\n\n').length-1;const key=n===3?'A B C':n===2?'A B':'A';return {tokens:c.counts[key]+1,basis:'exact'};}};
    const client=new tf.Tokenfold({select:f.select,counter});
    try {
      // Context adds one required query token to the format-neutral fixture's fixed-empty plan.
      const r=await client.fit({kind:'context',query:'query',groups:fixture.groups.map(g=>({id:g.id,content:g.scoring_text,required:g.required}))},{targetTokens:fixture.target_tokens+1,allowSelection:true});
      assert.deepEqual(r.groupsKept,c.groups_kept,c.id);assert.equal(r.finalTokens,c.final_tokens+1,c.id);assert.equal(r.reason,c.reason??null,c.id);
    } finally {await client.close();}
  }
});
test('close rejects an admitted waiting score and concurrent cold warmups own one child',async t=>{
  const f=runtimeFixture(t);if(!f)return;
  const client=new tf.Tokenfold({select:f.select});t.after(()=>client.close());
  await Promise.all([client.warmup(),client.warmup()]);
  assert.equal(readFileSync(join(f.root,'pids'),'utf8').trim().split('\n').length,1);
  const active=client.fit(context('hold'),{targetTokens:8,allowSelection:true});
  const activeRejected=assert.rejects(active,e=>e instanceof tf.CancelledError||e instanceof tf.ClosedError);
  await waitFor(()=>existsSync(join(f.root,'active')));
  const proto=native().NativeClient.prototype,begin=proto.begin;let admitted=false;
  proto.begin=async function(...args){const result=await begin.apply(this,args);if(JSON.parse(result.action).action==='score')admitted=true;return result;};
  let waiting;
  try {waiting=client.fit(context(),{targetTokens:8,allowSelection:true});await waitFor(()=>admitted);}finally{proto.begin=begin;}
  const waitingRejected=assert.rejects(waiting,tf.ClosedError);
  await client.close();await waitingRejected;await activeRejected;
});
