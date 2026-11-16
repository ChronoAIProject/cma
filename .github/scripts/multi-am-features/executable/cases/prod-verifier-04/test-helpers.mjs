import assert from 'node:assert/strict';
import {executeCase} from '../../runner.mjs';
import {cases} from './index.mjs';
import {rows} from './common.mjs';

const clone = structuredClone;
const config = {phase:'before_restart',iteration:1,am:'a',image:'model@sha256:'+'a'.repeat(64),source:'b'.repeat(40)};
const ownerAdapter = {
  async ownerOf({agentId}) {return {agent_id:agentId,am:'a',epoch:1,instance_id:'first',pod_uid:'pod-first'};},
  async topology() {return {shared_database:true,coordination_enabled:true,ams:{
    a:{ready:true,instance_id:'first',pod_uid:'pod-first'},b:{ready:true,instance_id:'second',pod_uid:'pod-second'}}};},
};
export async function execute(number, fixtureName, fixture, handler) {
  const definition = cases.find(value=>value.id.endsWith('-'+String(number).padStart(3,'0')));
  const calls = [], cleanups = [];
  let sequence = 0;
  const transport = {async request(input) {
    calls.push(clone(input));
    const response = await handler(input);
    if (!response) throw Error('test_unhandled_route');
    return {...response,body:clone(response.body),timing:{am:input.am,request_id:'model-'+sequence++,body_ms:1}};
  }};
  const result = await executeCase(definition,rows.find(row=>row.case_id===definition.id),{...config,transport,adapter:ownerAdapter,
    fixtures:{'api-fixtures':{async take(name) {
      assert.equal(name,fixtureName);
      return {...fixture,cleanup:async()=>{cleanups.push(name);await fixture.cleanup?.();}};
    }}}});
  return {result,calls,cleanups};
}
export function pass(value) {
  assert.equal(value.result.status,'PASS',value.result.primary_failure);
  assert.ok(value.result.steps.every(step=>step.status==='PASS' && step.assertions>0));
  assert.equal(value.cleanups.length,1);
}
