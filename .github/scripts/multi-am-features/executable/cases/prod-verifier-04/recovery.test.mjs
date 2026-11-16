import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';

test('recovery flows retain original identity through 202 uncertainty with one attachment',async()=>{
  for(const number of [13,14,15,16,17]) {
    const original={agent_id:'agt_recovery',creation_key:'create-original',creation_fingerprint:'a'.repeat(64),failure_seq:9,requested_at:'2026-11-16T00:00:00Z'};
    let phase=null,attached=0;
    const fixture={original,async makeNextAdmissionOutcomeUnknown(){},async readNativeFacts(){return {
      agent_id:original.agent_id,parent_id:'sbx_one',ever_ready:false,status:attached?'ready':'failed',
      data_digest:'d'.repeat(64),preparations:1,input_effects:0,attachments:attached,agent_ids:[original.agent_id]};}};
    const result=await execute(number,'capacity-recovery',fixture,async input=>{
      if(input.actor==='outsider') return {status:404,body:{code:'capacity_recovery_unavailable'}};
      if(input.body && JSON.stringify(input.body)!==JSON.stringify(original)) return {status:409,body:{code:'capacity_recovery_conflict'}};
      if(input.path.endsWith('/dry-run')) return {status:200,body:{agent_id:original.agent_id,disposition:'eligible',through_seq:9}};
      if(input.path.endsWith('/admit') && !phase) {phase='admitted';return {status:202,body:{outcome:'unknown'}};}
      if(input.path.endsWith('/resume')) {phase='completed';attached=1;}
      if(input.path.endsWith('/admit')||input.path.endsWith('/resume')) return {status:200,body:{outcome:'recorded',operation_key:'original-operation',phase,attempts:1}};
      return {status:200,body:{agent_id:original.agent_id,disposition:'eligible',attempts:phase?1:0,phase,operation_key:phase?'original-operation':null}};
    });
    pass(result);assert.equal(attached,1);
    assert.ok(result.result.observations.some(value=>value.label==='cross_am_read_attribution'));
  }
});
