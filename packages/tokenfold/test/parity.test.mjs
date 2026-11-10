import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import * as tf from '../dist/index.js';
import { runtimeFixture, context } from './runtime-fixture.mjs';

test('installed Python and native Node agree on full receipts except timings', t=>{
  if(!process.env.TOKENFOLD_TEST_PARITY_PYTHON) {t.skip('requires separately installed Python SDK via TOKENFOLD_TEST_PARITY_PYTHON');return;}
  return compare(t);
});
async function compare(t) {
  const runtime=runtimeFixture(t);if(!runtime)return;
  const core=JSON.parse(readFileSync(new URL('../../../tests/fixtures/sdk-fit/core.json',import.meta.url)));
  const provider=JSON.parse(readFileSync(new URL('../../../tests/fixtures/sdk-fit/openai.json',import.meta.url)));
  const cases=[
    {request:' unchanged Unicode 🐎\n',target:100,counter:'encoding'},
    {request:{kind:'json_data',value:{z:[1,-0],a:true}},target:100,counter:'encoding'},
    {request:core.request,target:core.target_tokens,counter:'fixture'},
    {request:{kind:'openai_chat',payload:provider.payload,groups:[{id:'history',indices:[1,2]}]},target:100,counter:'encoding'},
    {request:{kind:'openai_chat',payload:{model:'gpt-4o-2024-08-06',messages:[{role:'user',content:'Hello'}]}},target:100,counter:'provider',mode:'best_effort'},
    {request:context(),target:20,counter:'length',select:true},
    {request:context('nan'),target:20,counter:'length',select:true},
    {request:context(),target:20,counter:'estimated',select:true,mode:'best_effort'},
  ];
  const code=`
import json,sys,tokenfold as tf
data=json.load(sys.stdin)
def content(v): return tf.Data(v['value'],representation=v.get('representation','json')) if isinstance(v,dict) else v
def request(v):
 if isinstance(v,str): return v
 if v['kind']=='json_data': return content(v)
 if v['kind']=='context': return tf.Context(v['query'],[tf.Group(g['id'],content(g['content']),g.get('required',False)) for g in v['groups']])
 return tf.Request.openai(v['payload'],query=v.get('query'),groups=[tf.MessageGroup(g['id'],tuple(g['indices']),g.get('required',False)) for g in v.get('groups',[])])
class Counter:
 def __init__(self,kind): self.kind=kind;self.id=data['core']['counter_id'] if kind=='fixture' else 'parity:'+kind+':v1'
 def count(self,r):
  if self.kind=='fixture': return tf.TokenCount(**data['core']['original_count' if r.payload==data['core']['original_payload'] else 'proposal_count'])
  return tf.TokenCount(len(r.payload),'estimated' if self.kind=='estimated' else 'exact')
results=[]
for c in data['cases']:
 counter=tf.EncodingCounter() if c['counter']=='encoding' else tf.ProviderCounter.openai(model='gpt-4o-2024-08-06') if c['counter']=='provider' else Counter(c['counter'])
 with tf.Tokenfold(counter=counter,select=tf.SelectRuntime(data['config'],experimental=True) if c.get('select') else None) as client:
  r=client.fit(request(c['request']),target_tokens=c['target'],budget_mode=c.get('mode','strict'),allow_selection=c.get('select',False))
  receipt=json.loads(r.receipt.to_json());del receipt['timings_ms'];results.append(dict(payload=r.payload,receipt=receipt))
print(json.dumps(results,ensure_ascii=False))
`;
  const expected=JSON.parse(execFileSync(process.env.TOKENFOLD_TEST_PARITY_PYTHON,['-X','utf8','-c',code],{input:JSON.stringify({cases,core,config:runtime.select.config}),encoding:'utf8'}));
  const actual=[];
  for(const c of cases) {
    const counter=c.counter==='encoding'?new tf.EncodingCounter():c.counter==='provider'?tf.ProviderCounter.openai({model:'gpt-4o-2024-08-06'}):{id:c.counter==='fixture'?core.counter_id:`parity:${c.counter}:v1`,count:r=>c.counter==='fixture'?(r.payload===core.original_payload?core.original_count:core.proposal_count):{tokens:r.payload.length,basis:c.counter==='estimated'?'estimated':'exact'}};
    const client=new tf.Tokenfold({counter,...(c.select?{select:runtime.select}:{})});
    try {const r=await client.fit(c.request,{targetTokens:c.target,budgetMode:c.mode??'strict',allowSelection:c.select??false});const receipt=r.receipt.toJSON();delete receipt.timings_ms;actual.push({payload:r.payload,receipt});}
    finally{await client.close();}
  }
  assert.deepEqual(actual,expected);
}
