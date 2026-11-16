import test from 'node:test';
import assert from 'node:assert/strict';
import {executeCase} from './runner.mjs';

const row = {case_id: 'C275:EG-evidence', family: 'eg', required_steps: ['expected_result']};
const observation = () => ({schema: 'multi-am-eg-evidence-v1', secrets_redacted: true,
  event_sha256: '1'.repeat(64), response_sha256: '2'.repeat(64),
  requests: [{am: 'b', method: 'POST', status: 202, request_id: 'actual-am-request',
    agent_id_sha256: '3'.repeat(64), owner_proof: {kind: 'non_owner', verdict: 'PASS', owner: 'a', ingress: 'b',
      owner_epoch: 7, agent_sha256: '3'.repeat(64), request_id: 'actual-am-request'}}],
  eg_receipts: [{replica: 'a', request_id: 'different-eg-request'}]});
async function run(value) {
  return executeCase({id: row.case_id, scope: 'eg', fixtures: [], steps: row.required_steps,
    run: ctx => ctx.step('expected_result', async () => {
      await ctx.observe('eg_am_evidence', async () => value); ctx.assert(true, 'joined_actual_am_evidence');
    })}, row, {phase: 'before_restart', iteration: 1, am: 'a', source: '4'.repeat(40),
    image: 'local/api@sha256:' + '5'.repeat(64), campaignId: 'eg-evidence-contract'});
}

test('EG relay evidence imports actual AM requests without relabeling EG traffic', async () => {
  const result = await run(observation());
  assert.equal(result.status, 'PASS');
  assert.equal(result.requests.length, 1);
  assert.equal(result.requests[0].request_id, 'actual-am-request');
  assert.equal(result.owner_proofs[0].owner_epoch, 7);
});

test('EG labels, unmatched Agent/request identity and zero epochs cannot establish AM proof', async () => {
  for (const change of [value => value.requests = [], value => delete value.event_sha256,
    value => delete value.requests[0].agent_id_sha256,
    value => value.requests[0].owner_proof.request_id = 'eg-request',
    value => value.requests[0].owner_proof.agent_sha256 = '6'.repeat(64),
    value => value.requests[0].owner_proof.owner_epoch = 0,
    value => value.requests[0].owner_proof.owner = 'b']) {
    const value = observation(); change(value); assert.equal((await run(value)).status, 'FAIL');
  }
  const result = await run({schema: 'multi-eg-http-evidence-v1', scope: 'eg', request_id: 'eg-only'});
  assert.equal(result.requests.length, 0);
  assert.equal(result.owner_proofs.length, 0);
});
