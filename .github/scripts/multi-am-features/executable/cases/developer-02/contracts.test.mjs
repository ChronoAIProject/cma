import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {validReceipt,hash,closeThenCapture} from './helpers.mjs';
import {cases,setups,browserFromLease} from './index.mjs';
import {freshAgent} from './setups.mjs';
const source = 'a'.repeat(40);
function receipt(owner = 'b', kind = 'non_owner', method = 'POST') {
  const serving = {schema:'c275-serving-v1',am:'a',ready:true,origin:'http://127.0.0.1:10001',
    pod_uid:'pod-a',instance_id:'instance-a',container_id:'container-a',observed_at:'2026-11-16T10:00:00.000Z'};
  return {schema:'multi-am-browser-evidence-v1',real_ui:true,compiled_source:source,
    compiled_manifest:'sha256:'+'b'.repeat(64),secrets_redacted:true,requests:[{
      request_id:'request-1',am:'a',elapsed_ms:12,agent_id_sha256:hash('agent'),owner,
      serving,serving_after:{...serving,observed_at:'2026-11-16T10:00:00.003Z'},
      started_at:'2026-11-16T10:00:00.001Z',ended_at:'2026-11-16T10:00:00.002Z',
      attribution:{verdict:'PASS',basis:'pod_forward_before_after'},
      owner_epoch:3,owner_stable:true,method,owner_proof:{request_id:'request-1',ingress:'a',
        owner,owner_epoch:3,agent_sha256:hash('agent'),verdict:'PASS',kind}}]};
}
test('requires actual exact source and nonowner mutation proof',() => {
  assert.equal(validReceipt(receipt(),source),true);
  for (const mutate of [r => r.compiled_source = 'c'.repeat(40),r => r.requests[0].request_id = '',
    r => r.requests[0].owner_stable = false,r => r.requests[0].owner_proof.owner_epoch++,
    r => r.requests[0].owner_proof.request_id = 'different',r => r.secrets_redacted = false]) {
    const r = receipt(); mutate(r); assert.equal(validReceipt(r,source),false);
  }
});
test('explicit owner reads need peer-read proof; owner writes cannot borrow it',() => {
  assert.equal(validReceipt(receipt('a','peer_read','GET'),source),true);
  assert.equal(validReceipt(receipt('a','non_owner','GET'),source),false);
  assert.equal(validReceipt(receipt('a','peer_read','POST'),source),false);
});
test('definitions retain every original checklist step and stay inside assignment',() => {
  const assignment = JSON.parse(readFileSync(new URL('../../assignments/developer-02.json',import.meta.url)));
  const catalog = JSON.parse(readFileSync(new URL('../../catalog.json',import.meta.url)));
  assert.equal(new Set(cases.map(c => c.id)).size,cases.length);
  for (const c of cases) {
    assert(assignment.case_ids.includes(c.id));
    assert.deepEqual(c.steps,catalog.rows.find(r => r.case_id === c.id).required_steps);
    assert.deepEqual(c.fixtures,['browser','user-fixtures']);
  }
});
test('missing authentic fixture and browser lease block before any workload',async () => {
  for (const setup of Object.values(setups)) await assert.rejects(setup({},{}),/missing|unavailable/);
  assert.throws(() => browserFromLease({}),/lease_missing/);
  assert.throws(() => browserFromLease({scope:'LOCAL_DISPOSABLE_TWO_AM',parentOwnsBrowser:true}),/binding_missing/);
});
test('uncertain accepted creation retains receipt and never rekeys or repeats write',async () => {
  const deferred = [], calls = [], observations = [];
  const ctx = {am:'a',defer:(n,f) => deferred.push(f),observe:async(n,f) => observations.push(await f()),
    assert:(v,c) => {if (!v) throw Error(c);},request:async r => {
      calls.push(r);
      if (r.method === 'POST') throw Object.assign(Error('attribution_failed'),{response:{body:{id:'operation'}}});
      if (r.path.includes('agent-lifecycle')) return {body:{id:'operation',agent_id:'agent'}};
      if (r.path.includes('/agents/')) return {body:{id:'agent',sandbox:{id:'sandbox'}}};
      if (r.method === 'DELETE') return {status:202};
      return {body:{id:'sandbox',owner:'owner',status:calls.some(c => c.method === 'DELETE')?'retired':'active'}};
    }};
  const primitives = {'ui-runtime':{scope:'LOCAL_DISPOSABLE_TWO_AM',exclusive:true,principal:'owner',
    webSource:source,runtimeFor:async()=>{},finality:async()=>({agent_id:'agent',sandbox_id:'sandbox',
      children_remaining:0,cleanup_complete:true})}};
  await assert.rejects(freshAgent(ctx,primitives),/attribution_failed/);
  assert.equal(deferred.length,1); await deferred[0](); await deferred[0]();
  assert.equal(calls.filter(c => c.method === 'POST').length,1);
  assert.equal(calls.filter(c => c.method === 'DELETE').length,1);
});
test('browser closes and drains before final evidence, retaining both failures',async () => {
  const events=[];
  await closeThenCapture({close:async()=>events.push('close')},async()=>events.push('evidence'));
  assert.deepEqual(events,['close','evidence']);
  await assert.rejects(closeThenCapture({close:async()=>{events.push('close-fail');throw Error('late_mutation');}},
    async()=>events.push('evidence-ok')),/late_mutation/);
  await assert.rejects(closeThenCapture({close:async()=>{events.push('close-fail-both');throw Error('close_failure');}},
    async()=>{events.push('evidence-fail');throw Error('evidence_failure');}),error =>
    error instanceof AggregateError && error.errors.map(e=>e.message).sort().join(',') === 'close_failure,evidence_failure');
  assert.deepEqual(events.slice(-2),['close-fail-both','evidence-fail']);
});
