import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {randomBytes} from 'node:crypto';
import {Readable} from 'node:stream';
import test from 'node:test';
import {AgentSession} from './migration_agent_session.mjs';
import {AgentCleanup} from './migration_agent_cleanup.mjs';
import {LocalAgentApi} from './migration_agent_http.mjs';
import {LocalProviderControl} from './migration_agent_adapter.mjs';
import {runProtocol} from './migration_agent_protocol.mjs';
import {STAGES} from './migration_agent_journey.mjs';
import {sessionFixture,stageCommand} from './migration_agent_session_fixture.mjs';

async function integration(t,lostDelete=false) {
  const f=sessionFixture(),calls=[],streams=new Map(),retired=new Set();
  const bearer=randomBytes(24).toString('hex'),providerToken=randomBytes(24).toString('hex');
  const server=createServer(async(req,res)=>{
    const path=new URL(req.url,'http://127.0.0.1'),method=req.method;
    const bytes=[];for await(const chunk of req)bytes.push(chunk);
    const body=bytes.length ? JSON.parse(Buffer.concat(bytes)) : undefined;
    const provider=path.pathname.startsWith('/__ws4/');
    if(req.headers.authorization!=='Bearer '+(provider?providerToken:bearer)) {
      res.writeHead(401);res.end('{}');return;
    }
    calls.push({method,path:req.url});
    const json=(status,value)=>{res.writeHead(status,{'Content-Type':'application/json',
      'x-request-id':'actual-loopback-'+calls.length});res.end(status===204 ? undefined : JSON.stringify(value));};
    try {
      if(provider) {
        const caseId=body?.case_id ?? path.searchParams.get('case_id');
        if(path.pathname==='/__ws4/arm') await f.adapter.prepareTurn({caseId,marker:body.marker});
        if(path.pathname==='/__ws4/release') {
          await f.adapter.releaseTurn({caseId});
          const stream=streams.get(caseId),turn=f.state.turns.get(caseId);
          if(stream){stream.end('data: '+JSON.stringify({type:'response.completed',sequence_number:1,
            response:turn.response})+'\n\n');streams.delete(caseId);}
        }
        const state=await f.adapter.oracle({caseId});
        return json(200,{...state,released:state.completed_effect_count===1});
      }
      if(path.pathname.startsWith('/api/v2/agents/') && path.pathname.endsWith('/responses')) {
        const id=path.pathname.split('/')[4],key=req.headers['idempotency-key'];
        const stream=await f.api.send('a',id,body.input,key);
        res.writeHead(200,{'Content-Type':'text/event-stream','x-request-id':'actual-stream-'+calls.length});
        res.write('data: '+JSON.stringify({type:'response.created',sequence_number:0,
          response:{id:stream.responseId,conversation:{id},status:'in_progress'}})+'\n\n');
        streams.set(key,res);return;
      }
      if(method==='POST' && path.pathname.endsWith('/stop')) {
        const id=path.pathname.split('/')[4];f.state.records.get(id).detail.status='stopped';return json(202,{});
      }
      if(method==='DELETE' && path.pathname.startsWith('/api/v1/agents/')) {
        const id=path.pathname.split('/')[4];f.state.records.get(id).detail.status='deleted';
        if(lostDelete && id==='agent-1'){req.socket.destroy();return;}
        return json(204,null);
      }
      if(path.pathname==='/api/v1/agents' && method==='GET') {
        return json(200,{items:[...f.state.records.values()].map(r=>({id:r.id,status:r.detail.status,
          sandbox_id:r.detail.sandbox.id})),next:null});
      }
      if(path.pathname.startsWith('/api/v1/sandboxes/')) {
        const id=path.pathname.split('/')[4];
        if(method==='DELETE'){retired.add(id);return json(204,null);}
        const record=[...f.state.records.values()].find(r=>r.detail.sandbox.id===id);
        const suffix=' sandbox '+record.op;
        return json(200,{id,owner:'fixture-owner',name:record.fixtureName.slice(0,64-suffix.length)+suffix,
          status:retired.has(id)?'retired':'active'});
      }
      const response=await f.api.request('a',method,path.pathname,{body,key:req.headers['idempotency-key']});
      if(method==='POST' && path.pathname==='/api/v1/agents')
        [...f.state.records.values()].find(r=>r.op===response.body.id).fixtureName=body.name;
      return json(response.status,response.body);
    } catch {json(500,{error:'synthetic_fixture_failure'});}
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  t.after(async()=>{for(const stream of streams.values())stream.destroy();server.closeAllConnections();
    await new Promise(resolve=>server.close(resolve));});
  const origin='http://127.0.0.1:'+server.address().port;
  const api=new LocalAgentApi({origin,bearer,pacingMs:0,sleep:async()=>{},observe:async()=>{}});
  t.after(()=>api.close());
  const provider=new LocalProviderControl({origin,token:providerToken});
  const session=new AgentSession({...f.options,api,provider,
    cleanupFactory:options=>new AgentCleanup({...options,timeout:2000,now:()=>f.state.clock,
      sleep:async ms=>{f.state.clock+=ms;}})});
  const rows=[{sequence:0,op:'initialize',payload:{}},...STAGES.map((stage,i)=>({
    sequence:i+1,op:'stage',payload:{...stageCommand(stage),origin}})),{sequence:5,op:'finish',payload:{}}];
  const output=[];
  const result=await runProtocol({input:Readable.from([Buffer.from(rows.map(r=>JSON.stringify(r)+'\n').join(''))]),
    makeSession:async()=>session,write:async value=>output.push(value)});
  return {result,output,calls,retired,streams};
}

test('actual loopback API/SSE and public cleanup span all four session stages',async t=>{
  const r=await integration(t);
  assert.equal(r.result.exitCode,0);assert.equal(r.result.final.outcome,'SESSION_OBSERVED');
  assert.equal(r.result.final.cleanup.resources.length,2);assert.equal(r.retired.size,2);
  assert.equal(r.streams.size,0);
  assert.equal(r.calls.filter(c=>c.method==='POST' && c.path==='/api/v1/agents').length,2);
  assert.equal(r.calls.filter(c=>c.method==='POST' && c.path.endsWith('/responses')).length,6);
  assert.equal(r.calls.filter(c=>c.method==='DELETE').length,4);
});

test('actual lost delete response stays unresolved while the independent Agent is cleaned',async t=>{
  const r=await integration(t,true);
  assert.equal(r.result.exitCode,1);assert.equal(r.result.final.outcome,'INCOMPLETE');
  assert.equal(r.result.final.cleanup.resources[0].unknown_write,true);
  assert.equal(r.result.final.cleanup.resources[1].outcome,'PUBLIC_CLEANUP_OBSERVED');
  assert.equal(r.calls.filter(c=>c.method==='DELETE' && c.path==='/api/v1/agents/agent-1').length,1);
  assert.equal(r.calls.filter(c=>c.method==='DELETE' && c.path==='/api/v1/sandboxes/sandbox-1').length,0);
  assert.equal(r.retired.size,1);assert.equal(r.streams.size,0);
});
