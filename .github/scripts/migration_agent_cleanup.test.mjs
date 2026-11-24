import assert from 'node:assert/strict';
import test from 'node:test';
import {AgentCleanup} from './migration_agent_cleanup.mjs';
import {LocalAgentApi} from './migration_agent_http.mjs';

function fixture() {
  const resources=[1,2].map(n=>({key:'migration-key-'+n,name:'migration-name-'+n,
    operation_id:'operation-'+n,agent_id:'agent-'+n,lineage:{sandbox_id:'sandbox-'+n}}));
  const state={calls:[],events:[],agents:new Map(resources.map(r=>[r.agent_id,'ready'])),
    retired:new Set(),lose:null,mutate:()=>{},clock:0,observeFailure:null};
  const api={async request(am,method,path,options) {
    state.calls.push({am,method,path,options});
    const id=path.split('/')[4];let body,status=200;
    if(method==='POST') {state.agents.set(id,'stopped');status=202;body={};}
    else if(method==='DELETE') {
      if(path.includes('/agents/')) state.agents.set(id,'deleted');else state.retired.add(id);
      status=204;body=null;
    } else if(path.includes('/agent-lifecycle/')) body={id,agent_id:'agent-'+id.split('-').at(-1)};
    else if(path.includes('/agents?')) body={items:[...state.agents].map(([id,status])=>({
      id,status,sandbox_id:'sandbox-'+id.split('-').at(-1)})),next:null};
    else if(path.includes('/agents/')) body={id,status:state.agents.get(id),sandbox:{id:'sandbox-'+id.split('-').at(-1)}};
    else if(path.includes('/sandboxes/')) {
      const n=id.split('-').at(-1);
      body={id,owner:'fixture-owner',name:'migration-name-'+n+' sandbox operation-'+n,
        status:state.retired.has(id)?'retired':'active'};
    } else throw Error('unexpected_route');
    if(state.lose===method+' '+path) throw Error('local_transport_failed');
    const response={status,body,timing:{request_id:'request-'+state.calls.length}};
    state.mutate(response,method,path);return response;
  }};
  const record=async event=>{
    state.events.push(structuredClone(event));
    if(state.observeFailure===event.type) throw Error('journal_failed');
  };
  const options={api,owner:'fixture-owner',record,timeout:2000,now:()=>state.clock,
    sleep:async ms=>{state.clock+=ms;}};
  return {resources,state,api,options,cleanup:new AgentCleanup(options)};
}

test('public cleanup joins identities, records intents, stops/deletes/retires once',async()=>{
  const f=fixture(),result=await f.cleanup.run(f.resources);
  assert.equal(result.outcome,'PUBLIC_CLEANUP_OBSERVED');
  assert.equal(result.infrastructure_teardown,'NOT_PROVEN');
  assert.equal(f.state.calls.filter(c=>c.method!=='GET').length,6);
  assert.equal(f.state.events.filter(e=>e.type==='cleanup_write_intent').length,6);
  assert.ok(result.resources.every(r=>r.writes_observed===3 && !r.unknown_write));
  const stdout=JSON.stringify(result);
  for(const r of f.resources) assert.equal(stdout.includes(r.agent_id),false);
  await assert.rejects(f.cleanup.run(f.resources),/cleanup_once/);
});

test('lost mutation never retries and never permits dependent mutations',async()=>{
  for(const lost of ['POST /api/v1/agents/agent-1/stop','DELETE /api/v1/agents/agent-1',
    'DELETE /api/v1/sandboxes/sandbox-1']) {
    const f=fixture();f.state.lose=lost;
    const result=await f.cleanup.run(f.resources);
    assert.equal(result.outcome,'UNRESOLVED');
    assert.equal(result.resources[0].unknown_write,true);
    assert.equal(result.resources[1].outcome,'PUBLIC_CLEANUP_OBSERVED');
    const related=f.state.calls.filter(c=>c.method!=='GET' && /(?:agent|sandbox)-1(?:\/|$)/.test(c.path));
    assert.equal(related.at(-1).method+' '+related.at(-1).path,lost);
    assert.equal(related.filter(c=>c.method+' '+c.path===lost).length,1);
  }
});

test('ambiguous create is unresolved; known operation may resolve a partial admission',async()=>{
  const f=fixture();f.resources[0].operation_id=null;f.resources[0].agent_id=null;
  const result=await f.cleanup.run(f.resources);
  assert.equal(result.resources[0].failure,'cleanup_admission_identity_unresolved');
  assert.equal(result.resources[1].outcome,'PUBLIC_CLEANUP_OBSERVED');
  const g=fixture();g.resources[0].agent_id=null;delete g.resources[0].lineage;
  assert.equal((await g.cleanup.run([g.resources[0]])).outcome,'PUBLIC_CLEANUP_OBSERVED');
});

test('foreign lifecycle, Agent, Sandbox owner or name prevent mutation',async()=>{
  for(const field of ['operation','operation-agent','agent','sandbox','owner','name','missing-sandbox']) {
    const f=fixture();f.state.mutate=(r,method,path)=>{
      if(method!=='GET')return;
      if(field==='operation' && path.includes('/agent-lifecycle/'))r.body.id='other';
      if(field==='operation-agent' && path.includes('/agent-lifecycle/'))r.body.agent_id='other';
      if(field==='agent' && path.includes('/agents/'))r.body.id='other';
      if(field==='sandbox' && path.includes('/agents/'))r.body.sandbox.id='other';
      if(field==='missing-sandbox' && path.includes('/agents/'))r.body.sandbox=null;
      if(field==='owner' && path.includes('/sandboxes/'))r.body.owner='other';
      if(field==='name' && path.includes('/sandboxes/'))r.body.name='other';
    };
    assert.equal((await f.cleanup.run([f.resources[0]])).outcome,'UNRESOLVED',field);
    assert.equal(f.state.calls.filter(c=>c.method!=='GET').length,0,field);
  }
});

test('inventory must terminate without duplicates or another live Agent',async()=>{
  for(const mode of ['live-peer','missing-item','repeated-cursor','page-bound','bad-next','missing-items','duplicate-id']) {
    const f=fixture();let page=0;
    f.state.mutate=(r,method,path)=>{
      if(method!=='GET' || !path.includes('/agents?'))return;
      page++;
      if(mode==='live-peer')r.body.items.push({id:'another',sandbox_id:'sandbox-1',status:'ready'});
      if(mode==='missing-item')r.body.items=[{id:'missing'}];
      if(mode==='repeated-cursor')r.body={items:[],next:'cursor'};
      if(mode==='page-bound')r.body={items:[],next:'cursor-'+page};
      if(mode==='bad-next')r.body.next='unsafe?';
      if(mode==='missing-items')delete r.body.items;
      if(mode==='duplicate-id')r.body.items.push({...r.body.items[0]});
    };
    const result=await f.cleanup.run([f.resources[0]]);
    assert.equal(result.outcome,'UNRESOLVED',mode);
    assert.equal(f.state.calls.filter(c=>c.method==='DELETE' && c.path.includes('/sandboxes/')).length,0);
  }
  const f=fixture();let page=0;
  f.state.mutate=(r,_method,path)=>{if(path.includes('/agents?'))r.body={items:[],next:++page===1?'safe-cursor':null};};
  assert.equal((await f.cleanup.run([f.resources[0]])).outcome,'PUBLIC_CLEANUP_OBSERVED');
  assert.equal(page,2);
});

test('already stopped/deleted/retired resources are observed without repeat writes',async()=>{
  for(const status of ['stopped','deleted','missing']) {
    const f=fixture();f.state.agents.set('agent-1',status==='missing'?'deleted':status);
    f.state.retired.add('sandbox-1');
    if(status==='missing')f.state.mutate=(r,m,p)=>{if(m==='GET' && p==='/api/v1/agents/agent-1'){r.status=404;r.body=null;}};
    const result=await f.cleanup.run([f.resources[0]]);
    assert.equal(result.outcome,'PUBLIC_CLEANUP_OBSERVED');
    assert.equal(f.state.calls.filter(c=>c.method==='POST').length,0);
    assert.equal(f.state.calls.filter(c=>c.method==='DELETE').length,status==='stopped'?1:0);
  }
});

test('timeouts, changed IDs and failed journal preserve failure',async()=>{
  for(const mode of ['stop-timeout','delete-timeout','retire-timeout','stop-id','delete-id','retire-id']) {
    const f=fixture();
    f.state.mutate=(r,m,p)=>{
      if(m!=='GET')return;
      const wrote=f.state.calls.some(c=>c.method==='POST');
      if(p==='/api/v1/agents/agent-1' && wrote) {
        if(mode==='stop-timeout')r.body.status='ready';
        if(mode==='delete-timeout' && r.body.status==='deleted')r.body.status='stopped';
        if(mode==='stop-id')r.body.id='other';
        if(mode==='delete-id' && r.body.status==='deleted')r.body.id='other';
      }
      if(p==='/api/v1/sandboxes/sandbox-1' && f.state.retired.has('sandbox-1')) {
        if(mode==='retire-timeout')r.body.status='active';
        if(mode==='retire-id')r.body.owner='other';
      }
    };
    assert.equal((await f.cleanup.run([f.resources[0]])).outcome,'UNRESOLVED',mode);
  }
  const f=fixture();f.state.observeFailure='cleanup_write_intent';
  assert.equal((await f.cleanup.run(f.resources)).outcome,'UNRESOLVED');
  assert.equal(f.state.calls.filter(c=>c.method!=='GET').length,0);
  const g=fixture();g.state.observeFailure='cleanup_resource_finished';
  assert.equal((await g.cleanup.run(g.resources)).outcome,'UNRESOLVED');
});

test('cleanup list route only permits a bounded canonical inventory request',async()=>{
  const calls=[];const api=new LocalAgentApi({origin:'http://127.0.0.1:12345',bearer:'x'.repeat(32),
    observe:async()=>{},sleep:async()=>{},fetcher:async(url)=>{calls.push(url);return new Response('{"items":[]}');}});
  for(const path of ['/api/v1/agents?limit=200','/api/v1/agents?limit=200&cursor=abc-_='])
    await api.request('a','GET',path);
  for(const path of ['/api/v1/agents?limit=201','/api/v1/agents?limit=200&limit=200',
    '/api/v1/agents?limit=200&cursor=','/api/v1/agents?limit=200&owner=other',
    '/api/v1/agents?limit=200#fragment','/api/v1/agents?limit=200&cursor=x%2fy'])
    await assert.rejects(api.request('a','GET',path),/api_route_invalid/);
  assert.equal(calls.length,2);
});

test('duplicate custody and invalid limits are rejected before any cleanup request',async()=>{
  const f=fixture();
  await assert.rejects(f.cleanup.run([f.resources[0],structuredClone(f.resources[0])]),/duplicate_resource/);
  assert.equal(f.state.calls.length,0);
  const g=fixture();
  await assert.rejects(g.cleanup.run([{}, {}, {}]),/once_required/);
  for(const change of [{timeout:1},{owner:''},{record:null},{api:null}])
    assert.throws(()=>new AgentCleanup({...f.options,...change}),/contract_invalid/);
});
