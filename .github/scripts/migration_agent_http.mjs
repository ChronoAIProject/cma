import {performance} from 'node:perf_hooks';
import {identity, requireValue} from './migration_agent_observations.mjs';
import {consumeResponses} from './migration_agent_sse.mjs';

export function localOrigin(value) {
  requireValue(typeof value === 'string' && /^http:\/\/127\.0\.0\.1:[0-9]{1,5}$/.test(value),
    'local_origin_required');
  const parsed = new URL(value);
  requireValue(parsed.port && Number(parsed.port) > 0 && Number(parsed.port) <= 65535
    && parsed.origin === value, 'local_origin_required');
  return value;
}

export async function boundedBody(response, limit) {
  requireValue(response.body, 'response_body_missing');
  const chunks = []; let bytes = 0;
  for await (const chunk of response.body) {
    bytes += chunk.byteLength;
    requireValue(bytes <= limit, 'response_byte_limit');
    chunks.push(chunk);
  }
  return new TextDecoder('utf-8',{fatal:true}).decode(Buffer.concat(chunks));
}

export function safeFailure(error) {
  const code = error?.message;
  const known=new Set(['response_body_missing','response_byte_limit','send_not_sse','provider_control_shape_invalid',
    'sse_input_invalid','sse_after_done','sse_done_before_terminal','sse_sequence_invalid',
    'sse_admission_identity_invalid','sse_event_before_admission','sse_response_identity_changed',
    'sse_event_after_terminal','sse_terminal_status_mismatch','sse_terminal_failed',
    'sse_frame_limit','sse_byte_limit','sse_incomplete']);
  return new Error(known.has(code) || /^(?:api_status|send_status|provider_control_status)_[1-5][0-9]{2}$/.test(code ?? '')
    ? code : 'local_transport_failed');
}

function apiRoute(method,path) {
  requireValue(typeof path === 'string' && path.length < 512, 'api_route_invalid');
  if (method === 'GET' && path.startsWith('/api/v1/agents?')) {
    const parsed = new URL(path,'http://127.0.0.1');
    const keys = [...parsed.searchParams.keys()];
    requireValue(parsed.pathname === '/api/v1/agents' && !parsed.hash
      && parsed.searchParams.get('limit') === '200' && new Set(keys).size === keys.length
      && keys.every(key=>['limit','cursor'].includes(key))
      && (!keys.includes('cursor') || /^[a-zA-Z0-9_=-]{1,256}$/.test(parsed.searchParams.get('cursor'))),
    'api_route_invalid');
    return;
  }
  const id = '[-a-zA-Z0-9_]{1,160}';
  const patterns = {
    GET:new RegExp(`^/api/(?:v1/agents/${id}(?:/transcript)?|v1/agent-lifecycle/${id}|v1/sandboxes/${id}|v2/responses/${id})$`),
    POST:new RegExp(`^/api/v1/agents(?:/${id}/stop)?$`),
    DELETE:new RegExp(`^/api/v1/(?:agents|sandboxes)/${id}$`),
  };
  requireValue(patterns[method]?.test(path), 'api_route_invalid');
}

export class LocalAgentApi {
  #origin; #bearer; #observe; #fetch; #now; #sleep; #next = new Map(); #streams = new Set();
  #deadline; #pacing; #pending=0;
  constructor({origin,bearer,observe,deadlineMs = 240000,pacingMs = 6250,
    fetcher = fetch,now = () => performance.now(),sleep = ms => new Promise(resolve => setTimeout(resolve,ms))}) {
    this.#origin=localOrigin(origin);
    requireValue(typeof bearer === 'string' && /^[\x21-\x7e]{16,4096}$/.test(bearer), 'ephemeral_bearer_required');
    requireValue(typeof observe === 'function' && Number.isInteger(deadlineMs) && deadlineMs > 0
      && deadlineMs <= 240000 && Number.isFinite(pacingMs) && pacingMs >= 0, 'bounded_http_options_required');
    this.#bearer=bearer; this.#observe=observe; this.#deadline=deadlineMs; this.#pacing=pacingMs;
    this.#fetch=fetcher; this.#now=now; this.#sleep=sleep;
  }
  rebind(origin) {
    requireValue(this.#streams.size === 0 && this.#pending === 0, 'active_stream_prevents_reader_rebind');
    this.#origin=localOrigin(origin);
  }
  async #pace(method) {
    const tier=method === 'POST' ? 'write' : 'read', now=this.#now();
    const at=Math.max(now,this.#next.get(tier) ?? now);
    this.#next.set(tier,at+(tier === 'write' ? this.#pacing : 120));
    if (at>now) await this.#sleep(at-now);
  }
  #headers(body,key) {
    requireValue(key === undefined || identity(key), 'idempotency_key_invalid');
    return {Authorization:'Bearer '+this.#bearer,Host:'api.localhost.direct',Origin:'https://api.localhost.direct',
      ...(body === undefined ? {} : {'Content-Type':'application/json'}),...(key ? {'Idempotency-Key':key} : {})};
  }
  async #observation(response,method,path,started) {
    const value={method,route:path.replace(/\/(agents|sandboxes|agent-lifecycle|responses)\/[^/]+/g,'/$1/{id}'),
      status:response.status,request_id:response.headers.get('x-request-id'),elapsed_ms:this.#now()-started};
    await this.#observe(value); return value;
  }
  async request(am,method,path,{body,key,allowed=[200]} = {}) {
    requireValue(am === 'a', 'single_reader_required'); apiRoute(method,path);
    requireValue(Array.isArray(allowed) && allowed.length>0
      && allowed.every(n=>Number.isInteger(n)&&n>=200&&n<600), 'http_status_contract_invalid');
    const data=body === undefined ? undefined : JSON.stringify(body);
    requireValue(data === undefined || Buffer.byteLength(data)<=65536, 'request_byte_limit');
    const headers=this.#headers(body,key);
    this.#pending+=1;
    try {
      await this.#pace(method); const started=this.#now();
      const response=await this.#fetch(this.#origin+path,{method,headers,body:data,
        redirect:'error',signal:AbortSignal.timeout(Math.min(this.#deadline,30000))});
      const text=response.status === 204 ? '' : await boundedBody(response,4*1024*1024);
      const timing=await this.#observation(response,method,path,started);
      requireValue(allowed.includes(response.status),'api_status_'+response.status);
      return {status:response.status,body:text ? JSON.parse(text) : null,timing};
    } catch(error) {throw safeFailure(error);} finally {this.#pending-=1;}
  }
  async send(am,agentId,input,key) {
    requireValue(am === 'a' && identity(agentId) && typeof input === 'string'
      && input.length>0 && Buffer.byteLength(input)<=65536, 'response_request_invalid');
    const headers=this.#headers({},key);
    this.#pending+=1;
    try {await this.#pace('POST');}
    catch(error) {this.#pending-=1;throw safeFailure(error);}
    const controller=new AbortController(), timer=setTimeout(()=>controller.abort(),this.#deadline);
    const path='/api/v2/agents/'+agentId+'/responses',started=this.#now();
    let accept,refuse,created=false;
    const admitted=new Promise((resolve,reject)=>{accept=resolve;refuse=reject;});
    const handle={responseId:null,abort:()=>controller.abort(),done:null};
    this.#streams.add(handle);
    this.#pending-=1;
    handle.done=(async()=>{
      try {
        const response=await this.#fetch(this.#origin+path,{method:'POST',headers,
          body:JSON.stringify({input,stream:true}),redirect:'error',signal:controller.signal});
        await this.#observation(response,'POST',path,started);
        requireValue([200,202].includes(response.status),'send_status_'+response.status);
        requireValue(response.headers.get('content-type')?.split(';')[0].trim()==='text/event-stream', 'send_not_sse');
        await consumeResponses(response.body,agentId,responseId=>{
          handle.responseId=responseId; created=true; accept(handle);
        });
      } catch(error) {
        const failure=safeFailure(error);
        if (!created) refuse(failure);
        throw failure;
      } finally {clearTimeout(timer);this.#streams.delete(handle);}
    })();
    // Admission can reject before a caller receives the handle. Keep that
    // rejection observed without replacing the original completion promise.
    void handle.done.catch(()=>{});
    return admitted;
  }
  async close() {
    const streams=[...this.#streams];
    for (const stream of streams) stream.abort();
    const outcomes=await Promise.allSettled(streams.map(stream=>stream.done));
    return {joined:outcomes.length,failed:outcomes.filter(x=>x.status==='rejected').length};
  }
}
