import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {randomBytes} from 'node:crypto';
import {LocalAgentApi,localOrigin,boundedBody,safeFailure} from './migration_agent_http.mjs';
import {consumeResponses} from './migration_agent_sse.mjs';
import {LocalProviderControl,nativeAgentAdapter} from './migration_agent_adapter.mjs';
import {RetainedAgentJourney,STAGES} from './migration_agent_journey.mjs';
import {fixture} from './migration_agent_test_fixture.mjs';

const admitted={type:'response.created',sequence_number:0,
  response:{id:'response-one',conversation:{id:'agent-one'},status:'in_progress'}};
const complete={type:'response.completed',sequence_number:2,
  response:{id:'response-one',conversation:{id:'agent-one'},status:'completed'}};
const frame=(value,newline='\n')=>'data: '+JSON.stringify(value)+newline+newline;
async function* body(value,step=1) {for(let i=0;i<value.length;i+=step)yield Buffer.from(value.slice(i,i+step));}
async function server(t,handler) {
  const app=createServer(handler);await new Promise(resolve=>app.listen(0,'127.0.0.1',resolve));
  t.after(async()=>{app.closeAllConnections();await new Promise(resolve=>app.close(resolve));});
  return 'http://127.0.0.1:'+app.address().port;
}
function client(origin,changes={}) {
  const observations=[];
  return {observations,api:new LocalAgentApi({origin,bearer:randomBytes(24).toString('hex'),
    observe:async v=>observations.push(v),pacingMs:0,...changes})};
}

test('local HTTP roundtrip binds request IDs without logging bearer',async t=>{
  const requests=[],origin=await server(t,(req,res)=>{
    requests.push({method:req.method,path:req.url,authorization:req.headers.authorization});
    res.setHeader('x-request-id','request-one');res.end(JSON.stringify({id:'agent-one'}));
  });
  const {api,observations}=client(origin);t.after(()=>api.close());
  assert.equal((await api.request('a','GET','/api/v1/agents/agent-one')).body.id,'agent-one');
  assert.equal(observations[0].request_id,'request-one');
  assert.equal(observations[0].route,'/api/v1/agents/{id}');
  assert.ok(requests[0].authorization.startsWith('Bearer '));
  assert.equal(JSON.stringify(observations).includes(requests[0].authorization),false);
  await api.request('a','POST','/api/v1/agents',{body:{name:'test'},key:'one'});
  assert.equal(requests.length,2);
});

test('local transport refuses redirects, reports lost writes once, and redacts remote text',async t=>{
  let calls=0;
  const origin=await server(t,(_req,res)=>{calls++;res.writeHead(302,{Location:'https://example.invalid/private'});res.end();});
  const {api}=client(origin);t.after(()=>api.close());
  await assert.rejects(api.request('a','POST','/api/v1/agents',{key:'create',body:{}}),/local_transport_failed/);
  assert.equal(calls,1);
  const lost=client(origin,{fetcher:async()=>{calls++;throw new Error('credential content must not escape');}}).api;
  await assert.rejects(lost.send('a','agent-one','hello','one'),/local_transport_failed/);
  assert.equal(calls,2);assert.deepEqual(await lost.close(),{joined:0,failed:0});
});

test('actual SSE handles split CRLF and waits for final completion',async t=>{
  const origin=await server(t,(_req,res)=>{
    res.writeHead(200,{'Content-Type':'text/event-stream','x-request-id':'sse-one'});
    const text=frame(admitted,'\r\n')+': keepalive\r\n\r\n'+frame(complete,'\r\n')+'data: [DONE]\r\n\r\n';
    for (const char of text) res.write(char);
    res.end();
  });
  const {api,observations}=client(origin);t.after(()=>api.close());
  const stream=await api.send('a','agent-one','hello','turn-one');
  assert.equal(stream.responseId,'response-one');await stream.done;
  assert.equal(observations[0].request_id,'sse-one');
  api.rebind(origin);assert.deepEqual(await api.close(),{joined:0,failed:0});
});

test('stream interruption after admission is failed and joined',async t=>{
  let calls=0;
  const origin=await server(t,(_req,res)=>{calls++;res.writeHead(200,{'Content-Type':'text/event-stream'});
    res.write(frame(admitted));res.flushHeaders();});
  const {api}=client(origin);t.after(()=>api.close());
  const stream=await api.send('a','agent-one','hello','turn-one');
  assert.throws(()=>api.rebind(origin),/active_stream/);
  const closed=await api.close();
  assert.deepEqual(closed,{joined:1,failed:1});
  await assert.rejects(stream.done,/local_transport_failed/);
  assert.equal(calls,1);
});

test('SSE missing admission, empty body, failed terminal and truncation reject',async()=>{
  for (const [text,reason] of [
    ['',/sse_incomplete/],[frame(complete),/sse_event_before_admission/],
    [frame(admitted),/sse_incomplete/],
    [frame(admitted)+frame({...complete,type:'response.failed',response:{...complete.response,status:'failed'}}),/sse_terminal_failed/],
    [frame(admitted)+frame({...complete,response:{...complete.response,status:'in_progress'}}),/sse_terminal_status/],
    [frame(admitted)+frame(complete)+'truncated',/sse_incomplete/],
    [frame(admitted)+'data: [DONE]\n\n',/sse_done_before_terminal/],
    [frame(admitted)+frame({...admitted,sequence_number:1}),/sse_admission_identity/],
    [frame(admitted)+frame({...complete,sequence_number:0}),/sse_sequence/],
    [frame(admitted)+frame({...complete,response:{...complete.response,id:'wrong'}}),/sse_response_identity/],
    [frame(admitted)+frame({type:'response.delta',sequence_number:1,response_id:'wrong'}),/sse_response_identity/],
    [frame(admitted)+frame(complete)+frame({...complete,sequence_number:3}),/sse_event_after_terminal/],
    [frame(admitted)+frame(complete)+'data: [DONE]\n\ndata: [DONE]\n\n',/sse_after_done/],
  ]) await assert.rejects(consumeResponses(body(text),'agent-one',()=>{}),reason);
});

test('SSE enforces incremental frame, total-byte and UTF8 bounds',async()=>{
  await assert.rejects(consumeResponses(body('x'.repeat(200)),'agent-one',()=>{},{maxFrame:50}),/sse_frame_limit/);
  await assert.rejects(consumeResponses(body(frame(admitted),1000),'agent-one',()=>{},{maxFrame:50}),/sse_frame_limit/);
  await assert.rejects(consumeResponses(body(': keepalive\n\n'.repeat(40)),'agent-one',()=>{},{maxBytes:50}),/sse_byte_limit/);
  await assert.rejects(consumeResponses((async function*(){yield Buffer.from([0xff]);})(),'agent-one',()=>{}));
  let observed;
  await consumeResponses(body(frame(admitted)+frame({type:'response.delta',sequence_number:1,response_id:'response-one'})+frame(complete)),
    'agent-one',v=>{observed=v;});
  assert.equal(observed,'response-one');
});

test('HTTP rejection and malformed JSON do not disclose bodies',async()=>{
  for (const [response,reason] of [
    [new Response('private detail',{status:500}),/api_status_500/],
    [new Response('not JSON'),/local_transport_failed/],
    [new Response('x'.repeat(4*1024*1024+1)),/response_byte_limit/],
  ]) {
    const {api}=client('http://127.0.0.1:41000',{fetcher:async()=>response});
    await assert.rejects(api.request('a','GET','/api/v1/agents/one'),reason);
  }
  const {api}=client('http://127.0.0.1:41000',{fetcher:async()=>new Response(null,{status:204})});
  assert.equal((await api.request('a','DELETE','/api/v1/agents/one',{allowed:[204]})).body,null);
  await assert.rejects(boundedBody(new Response(null),10),/body_missing/);
});

test('origins, routes, request sizes and constructor options fail before transport',async()=>{
  for (const origin of ['http://localhost:4000','http://127.0.0.1:0','http://127.0.0.1:65536','http://127.0.0.1:04000',
    'https://127.0.0.1:4000','http://127.0.0.1:4000/','http://127.0.0.1:4000?x','http://u:p@127.0.0.1:4000'])
    assert.throws(()=>localOrigin(origin));
  const {api}=client('http://127.0.0.1:41000',{fetcher:()=>{throw new Error('must not call');}});
  for (const [am,method,path,options] of [['b','GET','/api/v1/agents/one'],['a','GET','https://example.invalid'],
    ['a','POST','/api/v1/admin/repair'],['a','GET','/api/v1/agents/../secret'],
    ['a','GET','/api/v1/agents/one',{allowed:[]}],['a','POST','/api/v1/agents',{key:'bad key'}],
    ['a','POST','/api/v1/agents',{body:'x'.repeat(65536)}]]) await assert.rejects(api.request(am,method,path,options));
  await assert.rejects(api.send('b','agent','hello','key'),/response_request/);
  assert.throws(()=>client('http://127.0.0.1:41000',{deadlineMs:Infinity}));
  assert.throws(()=>client('http://127.0.0.1:41000',{bearer:'short'}));
});

test('HTTP pacing persists across reader rebind without request retries',async()=>{
  let now=0;const delays=[];
  const {api}=client('http://127.0.0.1:41000',{pacingMs:6250,now:()=>now,sleep:async ms=>{delays.push(ms);now+=ms;},
    fetcher:async()=>new Response('{}')});
  await api.request('a','POST','/api/v1/agents');api.rebind('http://127.0.0.1:41001');
  await api.request('a','POST','/api/v1/agents');
  await api.request('a','GET','/api/v1/agents/one');await api.request('a','GET','/api/v1/agents/one');
  assert.deepEqual(delays,[6250,120]);
});

test('synthetic provider control and native adapter use exact Agent joins',async t=>{
  const paths=[],origin=await server(t,(req,res)=>{paths.push(req.url);res.end('{"ok":true}');});
  const provider=new LocalProviderControl({origin,token:randomBytes(24).toString('hex')});
  const native=[];const adapter=nativeAgentAdapter({provider,
    api:{request:async()=>({body:{id:'agent-one',sandbox:{id:'sandbox-one'}}})},
    observer:{reader:async()=>({ready:true}),runtimeFor:async value=>{native.push(value);return value;}}});
  await adapter.prepareTurn({caseId:'one',marker:'MARKER_TEST'});
  await adapter.releaseTurn({caseId:'one'});await adapter.oracle({caseId:'one'});
  assert.equal((await adapter.runtimeFor({agentId:'agent-one'})).sandboxId,'sandbox-one');
  assert.deepEqual(await adapter.reader(),{ready:true});
  assert.deepEqual(paths,['/__ws4/arm','/__ws4/release','/__ws4/state?case_id=one']);
  assert.deepEqual(native,[{agentId:'agent-one',sandboxId:'sandbox-one'}]);
  assert.throws(()=>provider.prepareTurn({caseId:'bad key',marker:'MARKER_TEST'}));
  assert.throws(()=>provider.oracle({caseId:'../bad'}));
  assert.throws(()=>provider.releaseTurn({caseId:null}));
  assert.throws(()=>new LocalProviderControl({origin,token:''}));
  const wrong=nativeAgentAdapter({provider,api:{request:async()=>({body:{id:'wrong'}})}});
  await assert.rejects(wrong.runtimeFor({agentId:'agent-one'}),/runtime_agent_join/);
  const failed=new LocalProviderControl({origin,token:randomBytes(24).toString('hex'),
    fetcher:async()=>new Response('private detail',{status:503})});
  await assert.rejects(failed.oracle({caseId:'one'}),/provider_control_status_503/);
});

test('all four stages use actual loopback HTTP/SSE and synthetic provider controls',async t=>{
  const f=fixture(),streams=new Map(),errors=[];
  const origin=await server(t,(req,res)=>{void (async()=>{
    try {
      const chunks=[];for await(const chunk of req)chunks.push(chunk);
      const text=Buffer.concat(chunks).toString();const data=text?JSON.parse(text):undefined;
      if (req.url.startsWith('/__ws4/')) {
        let result;
        if (req.url==='/__ws4/arm')result=await f.adapter.prepareTurn({caseId:data.case_id,marker:data.marker});
        else if(req.url==='/__ws4/release') {
          await f.adapter.releaseTurn({caseId:data.case_id});
          const pending=streams.get(data.case_id);
          const response=f.state.turns.get(data.case_id).response;
          pending.write(frame({type:'response.completed',sequence_number:1,response}));pending.end();
          streams.delete(data.case_id);result={ok:true};
        } else result=await f.adapter.oracle({caseId:new URL(req.url,origin).searchParams.get('case_id')});
        res.end(JSON.stringify(result??{ok:true}));return;
      }
      const send=req.url.match(/^\/api\/v2\/agents\/([^/]+)\/responses$/);
      if(send) {
        const key=req.headers['idempotency-key'];
        const receipt=await f.api.send('a',send[1],data.input,key);
        streams.set(key,res);res.writeHead(202,{'Content-Type':'text/event-stream','x-request-id':key});
        res.write(frame({type:'response.created',sequence_number:0,response:{id:receipt.responseId,
          conversation:{id:send[1]},status:'in_progress'}}));return;
      }
      const response=await f.api.request('a',req.method,req.url,{body:data,key:req.headers['idempotency-key']});
      res.writeHead(response.status,{'Content-Type':'application/json','x-request-id':response.timing.request_id});
      res.end(JSON.stringify(response.body));
    } catch(error) {errors.push(error);res.writeHead(500);res.end('{}');}
  })();});
  const {api}=client(origin,{sleep:async()=>{},now:()=>0});t.after(()=>api.close());
  const provider=new LocalProviderControl({origin,token:randomBytes(24).toString('hex')});
  const adapter=nativeAgentAdapter({api,provider,observer:{reader:()=>f.adapter.reader(),
    runtimeFor:args=>f.adapter.runtimeFor(args)}});
  const journey=new RetainedAgentJourney({...f.options,api,adapter});
  const results=[];
  for(const [index,stage]of STAGES.entries()) {
    f.switchReader(index===0||index===3?'old':'new',index===0?0:index===3?2:1);
    api.rebind(origin);results.push(await journey.run(stage));
  }
  assert.deepEqual(results.map(r=>r.agents.map(a=>a.responses)),[[1],[2],[3,1],[4,2]]);
  assert.equal(streams.size,0);assert.deepEqual(errors,[]);
  assert.equal(f.state.calls.filter(c=>c.method==='SEND').length,6);
});

test('HTTP and SSE deadlines abort a stalled local endpoint',async t=>{
  const origin=await server(t,(_req,_res)=>{});
  const {api}=client(origin,{deadlineMs:20});t.after(()=>api.close());
  await assert.rejects(api.request('a','GET','/api/v1/agents/one'),/local_transport_failed/);
  await assert.rejects(api.send('a','agent-one','hello','turn'),/local_transport_failed/);
  assert.deepEqual(await api.close(),{joined:0,failed:0});
});

test('pending reads and paced sends block reader rebind',async()=>{
  let finish;
  const {api}=client('http://127.0.0.1:41000',{fetcher:async()=>new Promise(resolve=>{finish=resolve;})});
  const pending=api.request('a','GET','/api/v1/agents/one');
  await new Promise(resolve=>setImmediate(resolve));
  assert.throws(()=>api.rebind('http://127.0.0.1:41001'),/active_stream/);
  finish(new Response('{}'));await pending;api.rebind('http://127.0.0.1:41001');
  let unpace;
  const delayed=client('http://127.0.0.1:41000',{pacingMs:6250,now:()=>0,
    sleep:async()=>new Promise(resolve=>{unpace=resolve;}),fetcher:async()=>new Response('{}')}).api;
  await delayed.request('a','POST','/api/v1/agents');
  const send=delayed.send('a','agent-one','hello','turn');
  assert.throws(()=>delayed.rebind('http://127.0.0.1:41001'),/active_stream/);
  unpace();await assert.rejects(send,/send_not_sse/);
  delayed.rebind('http://127.0.0.1:41001');
});

test('provider observations discard unrelated returned headers',async()=>{
  const control=new LocalProviderControl({origin:'http://127.0.0.1:41000',token:randomBytes(24).toString('hex'),
    fetcher:async()=>new Response(JSON.stringify({case_id:'case',request_count:1,
      authorization:'sensitive-fixture-marker',request_headers:{private:'omitted'}}))});
  assert.deepEqual(await control.oracle({caseId:'case'}),{case_id:'case',request_count:1});
  assert.equal(safeFailure(new Error('opaque_credential_value')).message,'local_transport_failed');
});
