import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';

test('hierarchy flows use nested Agent creation shapes and clean children before parents',async()=>{
  for(const number of [33,62,63,81,82]) {
    const defaults={sandbox_profile:{id:'sbp',revision:1},workspace_profile:{id:'wkp',revision:1},agent_profile:{id:'agp',revision:1}};
    const cleanupOrder=[],receipts=new Map();
    const created={};
    const fixture={defaults,sandboxRequest:{sandbox_profile:defaults.sandbox_profile},workspaceRequest:{workspace_profile:defaults.workspace_profile},
      agentRequest:{agent_profile:defaults.agent_profile},async waitForOwner(){},
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
  }
});
