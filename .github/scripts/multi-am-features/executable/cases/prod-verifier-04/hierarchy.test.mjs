import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';
import {prepareHierarchy} from './setups.mjs';
import {hash} from './common.mjs';

test('hierarchy flows use nested Agent creation shapes and clean children before parents',async()=>{
  for(const number of [33,62,63,81,82]) {
    const defaults={sandbox_profile:{id:'sbp',revision:1},workspace_profile:{id:'wkp',revision:1},agent_profile:{id:'agp',revision:1}};
    const cleanupOrder=[],receipts=new Map();
    const created={};
    const retained=[];
    const fixture={defaults,sandboxRequest:{sandbox_profile:defaults.sandbox_profile},workspaceRequest:{workspace_profile:defaults.workspace_profile},
      agentRequest:{agent_profile:defaults.agent_profile},async waitForOwner(){},
      async retainIntent(intent){retained.push(['intent',intent.kind]);},
      async retainResource(resource){retained.push(['resource',resource.kind]);},
      async cleanupResource(ctx,kind,id){cleanupOrder.push(kind);assert.equal(created[kind].id,id);},
      async readNativeFacts(){return {sandbox_ids:created.sandbox?['sbx']:[],workspace_ids:created.workspace?['wks']:[],
        agent_ids:created.agent?['agt']:[],agent_preparations:created.agent?1:0};}};
    const worktree={id:'wkt',workspace_id:'wks',sandbox_id:'sbx',status:'ready'};
    const result=await execute(number,'hierarchy',fixture,async input=>{
      if(input.actor==='outsider') return {status:404,body:{code:'resource_not_found'}};
      if(input.path==='/api/v1/defaults') return {status:200,body:defaults};
      if(input.method==='POST') {
        if(receipts.has(input.key)) return {status:201,body:receipts.get(input.key)};
        let kind,body;
        if(input.path==='/api/v1/sandboxes') {kind='sandbox';body={id:'sbx',profile:defaults.sandbox_profile,status:'active'};}
        else if(input.path.endsWith('/workspaces')) {kind='workspace';body={id:'wks',sandbox_id:'sbx',profile:defaults.workspace_profile,status:'ready'};}
        else {kind='agent';body={id:'agt',status:'ready',sandbox:{id:'sbx'},workspace:{id:'wks'},worktree:{id:'wkt'}};}
        created[kind]=body;receipts.set(input.key,body);return {status:201,body};
      }
      if(input.path.includes('/worktrees/')) return {status:200,body:worktree};
      return {status:200,body:input.path.includes('/agents/')?created.agent:
        input.path.includes('/workspaces/')?created.workspace:created.sandbox};
    });
    pass(result);assert.deepEqual(cleanupOrder,['agent','workspace','sandbox']);
    assert.deepEqual(retained,[['intent','sandbox'],['resource','sandbox'],['intent','workspace'],['resource','workspace'],['intent','agent'],['resource','agent']]);
    const worktreeReads=result.events.filter(event=>event.kind==='response' && event.request.path.includes('/worktrees/'));
    assert.ok(worktreeReads.length>=3);
    for(const event of worktreeReads) assert.ok(result.result.owner_proofs.some(proof=>
      proof.request_id===event.response.timing.request_id && proof.agent_sha256===hash('agt') && proof.verdict==='PASS'));
  }
});

test('hierarchy cleanup includes the Agent identity on every Agent-bound request', async()=>{
  const defaults={sandbox_profile:{id:'sbp',revision:1},workspace_profile:{id:'wkp',revision:1},agent_profile:{id:'agp',revision:1}};
  const calls=[]; let released=0, reads=0;
  const parent={scope:'LOCAL_DISPOSABLE_TWO_AM',database_disposable:true,single_matrix_writer:true,
    defaults,sandbox_profile:defaults.sandbox_profile,workspace_profile:defaults.workspace_profile,agent_profile:defaults.agent_profile,
    async reserve(){return {parent_cleanup_barrier:true,async retainIntent(){},async retainResource(){},async release(){released++;return {released:true};}};},
    async finality(){return {sandbox_id:'sbx',children_remaining:0,pending_operations:0,cleanup_complete:true};},
    async readNativeFacts(){return {};},async waitForOwner(){}};
  const ctx={row:{case_id:'hierarchy-cleanup-fixture'},am:'a',defer(label,operation){this.cleanup=operation;},
    async request(input){calls.push(input);reads++;return {status:200,body:{id:'agt',status:reads===1?'ready':'deleted'},timing:{am:input.am,request_id:'r'+reads}};}};
  const fixture=await prepareHierarchy(ctx,{userResources:parent});
  await fixture.retainResource({kind:'agent',id:'agt',key:'k'});
  await fixture.cleanupResource(ctx,'agent','agt');
  assert.deepEqual(calls.map(call=>call.agentId),['agt','agt','agt']);
  await fixture.cleanup();assert.equal(released,1);
});
