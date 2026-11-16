import test from 'node:test';
import assert from 'node:assert/strict';
import {executeCase} from './runner.mjs';
import {latencyVerdict} from './latency.mjs';

const row={case_id:'C275:cma-user.agent.stop',family:'user',required_steps:['action'],target_seconds:1};
async function run(action){
  const definition={id:row.case_id,scope:'global',steps:row.required_steps,fixtures:[],
    run:ctx=>ctx.step('action',async()=>{
      await action(ctx);ctx.assert(true,'action_observed');
      await ctx.observe('unit_action_evidence',async()=>({synthetic_unit_test:true}));
    })};
  return executeCase(definition,row,{phase:'steady',iteration:1,am:'a',
    source:'a'.repeat(40),image:'local/api@sha256:'+'b'.repeat(64)});
}

test('an unfinished actual runner timer remains a failed attempt even when steps return normally',async()=>{
  const result=await run(ctx=>{ctx.startAction('stop_to_terminal');});
  assert.equal(result.functional_status,'PASS');
  assert.equal(result.status,'FAIL');assert.equal(result.performance,'FAIL');
  assert.equal(result.action_timings.length,1);assert.equal(result.action_timings[0].status,'FAIL');
});

test('failed action callbacks remain in the original denominator alongside successful callbacks',async()=>{
  const result=await run(async ctx=>{
    await ctx.measureAction('stop_to_terminal',async()=>{});
    await assert.rejects(ctx.measureAction('stop_to_terminal',async()=>{throw Error('public_stop_refused');}),/public_stop_refused/);
    await ctx.measureAction('stop_to_terminal',async()=>{});
  });
  assert.deepEqual(result.action_timings.map(sample=>sample.status),['PASS','FAIL','PASS']);
  assert.equal(result.status,'FAIL');assert.equal(result.performance,'FAIL');
});

test('a public failure preserves primary error and fails its pending Stop sample',async()=>{
  const result=await run(ctx=>{ctx.startAction('stop_to_terminal');throw Error('terminal_read_failed');});
  assert.equal(result.primary_failure,'terminal_read_failed');
  assert.equal(result.status,'FAIL');assert.equal(result.action_timings.length,1);
  assert.equal(result.action_timings[0].status,'FAIL');
});

test('only matching completed samples at the explicit boundary satisfy the target',()=>{
  const sample={boundary:'stop_to_terminal',status:'PASS',elapsed_ms:1000};
  const verdict=samples=>latencyVerdict(row,{action_timings:samples});
  assert.equal(verdict([sample]),'PASS');
  assert.equal(verdict([]),'MISSING');
  assert.equal(verdict([{...sample,boundary:'setup_send_to_acknowledgement'}]),'MISSING');
  for(const elapsed_ms of [-1,NaN,Infinity,1000.01])assert.equal(verdict([{...sample,elapsed_ms}]),'FAIL');
  assert.equal(verdict([sample,{...sample,status:'FAIL'}]),'FAIL');
});
