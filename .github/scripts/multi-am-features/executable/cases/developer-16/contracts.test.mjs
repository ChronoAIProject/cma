import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {executeCase} from '../../runner.mjs';
import {loadRegistry} from '../../registry.mjs';
import {cases} from './index.mjs';
import {setup,finish,hash} from './helpers.mjs';

test('only eight assigned usage rows register and missing fixtures cannot run traffic',async()=>{
 const registry=await loadRegistry();
 const assigned=JSON.parse(readFileSync(new URL('../../assignments/developer-16.json',import.meta.url)));
 assert.deepEqual(cases.map(c=>c.id).sort(),assigned.case_ids.slice().sort());
 for(const definition of cases){
  const row=registry.rows.get(definition.id);assert.deepEqual(definition.steps,row.required_steps);
  const result=await executeCase(definition,row,{phase:'before_restart',iteration:1,am:'a',image:'local/api@sha256:'+'a'.repeat(64),source:'b'.repeat(40),fixtures:{}});
  assert.equal(result.status,'BLOCKED');assert.equal(result.requests.length,0);
 }
});
test('invalid local setup retains cleanup before refusal and never opens a browser',async()=>{
 let release;let opened=false;
 const ctx={fixture:name=>name==='user-fixtures'?{take:async()=>({local:false,run_owned:true,cleanup:async()=>({complete:true,owned_only:true})})}:{open:async()=>{opened=true;}},
  defer:(_label,fn)=>{release=fn;},observe:async(_label,fn)=>fn(),state:{}};
 await assert.rejects(setup(ctx,'usage'),/usage_local_owned_fixture_required/);
 assert.equal(opened,false);assert.equal(typeof release,'function');await release();
});
test('cleanup cannot convert incomplete resource disposal into success',async()=>{
 let release;
 const ctx={fixture:()=>({take:async()=>({local:false,cleanup:async()=>({complete:false,owned_only:true})})}),defer:(_label,fn)=>{release=fn;},state:{}};
 await assert.rejects(setup(ctx,'usage'),/usage_local_owned_fixture_required/);
 await assert.rejects(release(),/fixture_cleanup_incomplete/);
});
test('browser receipts are observed and mutation owner proof is enforced',async()=>{
 const source='a'.repeat(40),page={},agent='agent-fixture';let observed=0;
 const receipt={schema:'multi-am-browser-evidence-v1',real_ui:true,compiled_source:source,secrets_redacted:true,
  requests:[{request_id:'fixture-request',method:'POST',am:'b',elapsed_ms:1,agent_id_sha256:hash(agent),owner:'a',owner_epoch:2,owner_stable:true}]};
 const ctx={state:{f:{webSource:source},sessions:[{page,agentId:agent}]},fixture:()=>({evidence:async got=>{assert.equal(got,page);return receipt;}}),
  observe:async(label,fn)=>{assert.equal(label,'browser_receipt');observed++;return fn();},assert:(ok,code)=>assert.ok(ok,code)};
 await finish(ctx);assert.equal(observed,1);
 receipt.requests[0].owner='b';await assert.rejects(finish(ctx),/browser_current_non_owner_required/);
 receipt.requests[0].method='GET';await finish(ctx);
});
