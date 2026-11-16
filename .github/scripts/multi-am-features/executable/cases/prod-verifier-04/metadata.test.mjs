import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';
import {hash} from './common.mjs';

test('metadata flows use non-owner reads and retain independent artifact and lineage facts',async()=>{
  for(const number of [12,18,19,20,21,22,23]) {
    const fixture={agentId:'agt_meta',owner:'owner',created_at:'2026-11-16T00:00:00Z',sandboxId:'sbx',workspaceId:'wks',worktreeId:'wkt',
      artifact:{id:'art',content:'synthetic',name:'sample.txt',media_type:'text/plain',sha256:hash('synthetic')},
      feedback:{message_id:'msg',vote:'up',version:1,reasons:[],comment:null,request_id:'req'},
      provenance:{creation_origin:'manual',details:{}},associations:[{sandbox_id:'sbx',workspace_id:'wks',worktree_id:'wkt'}],
      source:{origin:{kind:'manual'},trigger:null},async readNativeFacts(){return {agent_id:'agt_meta',input_effects:0};}};
    const result=await execute(number,'agent-metadata',fixture,async input=>{
      if(input.actor==='outsider') return {status:404,body:{code:'agent_not_found'}};
      const suffix=input.path.split('/').at(-1);
      let body;
      if(suffix==='wkt') body={id:'wkt',workspace_id:'wks'};
      else if(suffix==='art') body={...fixture.artifact,agent_id:'agt_meta',size_bytes:9};
      else if(suffix==='deliverable-references') body={marker:'synthetic-marker',reference:{agent_id:'agt_meta',
        resource:{kind:'agent_artifact',id:'art'},sha256:fixture.artifact.sha256,size_bytes:9}};
      else if(suffix==='feedback') body={agent_id:'agt_meta',viewer_id:'owner',...fixture.feedback};
      else if(suffix==='provenance') body={agent_id:'agt_meta',created_at:fixture.created_at,...fixture.provenance};
      else if(suffix==='provider-associations') body={agent_id:'agt_meta',owner:'owner',agent_created_at:fixture.created_at,items:fixture.associations,next:null};
      else if(suffix==='source') body={agent_id:'agt_meta',...fixture.source};
      else return {status:501,body:{code:'capability_unsupported',capability:'terminal'}};
      return {status:200,body};
    });
    pass(result);
    assert.ok(result.result.owner_proofs.some(proof=>proof.owner!==proof.ingress));
    assert.ok(result.result.observations.some(value=>value.label==='cross_am_read_attribution'));
    const worktreeReads=result.events.filter(event=>event.kind==='response' && event.request.path.includes('/worktrees/'));
    assert.equal(worktreeReads.length,1);
    for(const event of worktreeReads) assert.ok(result.result.owner_proofs.some(proof=>
      proof.request_id===event.response.timing.request_id && proof.agent_sha256===hash(fixture.agentId) && proof.verdict==='PASS'));
  }
});
