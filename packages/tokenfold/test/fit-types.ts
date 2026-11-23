import { fit, Tokenfold, Receipt, EncodingCounter, ProviderCounter, type Data, type Context, type Request, type JsonObject, type FitResult, type MessageGroup, type TokenCounter } from 'tokenfold';
const data: Data = {kind:'json_data', value:{z:1,a:true}};
const context: Context = {kind:'context',query:'q',groups:[{id:'a',content:{...data}},{id:'b',content:'text',required:true}]};
const groups: MessageGroup[] = [];
const messages = [{role:'system',content:'instructions'}];
const start = messages.length;
messages.push({role:'assistant',content:'older evidence'});
groups.push({id:'history',indices:[start]});
messages.push({role:'user',content:'question'});
const request: Request = {kind:'openai_chat',payload:{model:'model',messages},groups};
const text: Promise<FitResult<string>> = fit('text',{targetTokens:20});
const structured: Promise<FitResult<string>> = fit({...data},{targetTokens:20});
const contextual: Promise<FitResult<string>> = fit({...context},{targetTokens:20});
const provider: Promise<FitResult<JsonObject>> = fit({...request},{targetTokens:20});
const counter: TokenCounter = {id:'custom:v1', async count(r) { return {tokens:JSON.stringify(r.payload).length,basis:'exact'}; }};
const client = new Tokenfold({counter,select:{config:'approval.json',experimental:true}});
const precise: Promise<FitResult<JsonObject>> = client.fit(request,{targetTokens:100,allowSelection:true,signal:new AbortController().signal});
const encoding = new EncodingCounter('cl100k_base');
const profile = ProviderCounter.openai({model:'gpt-4o-2024-08-06'});
async function check() {
  const r = await provider;
  const value: JsonObject = r.requireFit();
  const count: number | null = r.receipt.finalTokens;
  const id: string = r.receipt.accounting.counterId;
  const queue: number | null = r.receipt.timingsMs.scoreQueue;
  const hash: string | undefined = r.receipt.scorer?.manifestHash;
  Receipt.fromJSON(r.receipt.toJSON());
  // @ts-expect-error wrong payload assumption
  const wrong: string = r.payload;
  // @ts-expect-error immutable evidence
  r.receipt.accounting.counterId = 'changed';
  // @ts-expect-error wire fields do not leak into views
  r.receipt.final_tokens;
}
// @ts-expect-error callers cannot assert output using a generic argument
fit<JsonObject>('text',{targetTokens:10});
// @ts-expect-error plain text never returns provider payload
const wrong: Promise<FitResult<JsonObject>> = fit('text',{targetTokens:10});
// @ts-expect-error unknown data representation
fit({kind:'json_data',value:{},representation:'xml'},{targetTokens:10});
void [text,structured,contextual,precise,encoding,profile,check,wrong];
