import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';

test('delegation flows preserve scoped claims, original intent receipts and profile cleanup',async()=>{
  for(const number of [1,2,64,65]) {
    let variant='valid',profileCreates=0,handoffCreates=0,retired=false,state='pending';
    const receipts=new Map();
    const fixture={eventRef:'synthetic-reference',subject:'owner',grantId:'grant',profileRequest:{name:'synthetic',content:{system_prompt:'safe'}},
      connectionRequest:{launch_id:'launch',service_id:'service'},hostedOrigin:'http://localhost',
      async readNativeFacts(){return {profile_creates:profileCreates,handoff_creates:handoffCreates,credential_scopes:['bound-resource']};},
      async withBindingVariant(next,operation){variant=next;try{return await operation();}finally{variant='valid';}},
      async setBrokerHandoffState(id,next){assert.equal(id,'handoff');state=next;}};
    const profile={id:'profile',owner:'owner',draft:fixture.profileRequest.content};
    const result=await execute(number,'delegation',fixture,async input=>{
      if(input.actor==='outsider') return {status:404,body:{code:'service_connection_not_found'}};
      if(variant!=='valid') return {status:variant==='provider_unavailable'?503:403,body:{code:'delegation_refused'}};
      if(input.path.endsWith('/identity')) return {status:200,body:{subject:'owner',grant_id:'grant',login_url:null,display_name:'Synthetic'}};
      if(input.path.endsWith('/operations')) {
        if(input.body.path.includes('/admin/')) return {status:400,body:{code:'delegation_path_refused'}};
        if(!receipts.has(input.body.idempotency_key)) {profileCreates++;receipts.set(input.body.idempotency_key,{status:201,body:profile});}
        return {status:200,body:receipts.get(input.body.idempotency_key)};
      }
      if(input.path==='/api/v1/service-connections') {
        if(!receipts.has(input.key)) {handoffCreates++;receipts.set(input.key,{handoff_id:'handoff',hosted_url:'http://localhost/handoff',expires_at:'2026-11-16T01:00:00Z'});}
        return {status:201,body:receipts.get(input.key)};
      }
      if(input.path.endsWith('/handoff')) return {status:200,body:{handoff_id:'handoff',launch_id:'launch',service_id:'service',state}};
      if(input.path.endsWith('/retire')) {retired=true;return {status:200,body:{...profile,retired_at:'2026-11-16T00:00:00Z'}};}
      return {status:200,body:profile};
    });
    pass(result);assert.equal(profileCreates,1);assert.equal(handoffCreates,1);assert.equal(retired,true);assert.equal(variant,'valid');
  }
});
