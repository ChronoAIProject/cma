import test from 'node:test';
import assert from 'node:assert/strict';
import {coverageCounts} from './counts.mjs';
import {validEgReceipt, validPlatformProof, platformComplete} from './evidence-scope.mjs';

const source = '1'.repeat(40), image = 'fixture@sha256:' + '2'.repeat(64);
const binding = {source, image, campaign_id: 'qualified-ingress', fixtures: [],
  eg_source: source, eg_image: image,
  owner_restart: {verified: true, evidence_sha256: '3'.repeat(64)}};

function receipt(slot, eg) {
  const serving = {schema: eg ? 'c275-eg-serving-v1' : 'c275-serving-v1',
    ...(eg ? {scope: 'eg', replica: slot, source, image} : {am: slot}), ready: true, draining: false,
    origin: `http://127.0.0.1:${slot === 'a' ? 1111 : 2222}`, pod_uid: 'pod-' + slot,
    instance_id: 'instance-' + slot, container_id: 'container-' + slot, observed_at: '2026-11-16T10:00:00.000Z'};
  return {...(eg ? {scope: 'eg', replica: slot} : {am: slot}), request_id: 'request-' + slot,
    serving, started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}

function fixture(eg) {
  const row = {case_id: eg ? 'C275:EG-077' : 'C275:qualified-am', family: eg ? 'eg' : 'user',
    required_steps: ['expected_result'], phase_minimum_runs: {steady: 3}};
  const definition = {id: row.case_id, fixtures: [], scope: eg ? 'eg' : 'global',
    ...(eg ? {evidence_scope: 'eg_only'} : {})};
  const registry = {catalog: {rows: [row]}, cases: new Map([[row.case_id, definition]])};
  const sample = (iteration, slot) => ({id: row.case_id, family: row.family, source, image,
    campaign_id: binding.campaign_id, phase: 'steady', status: 'PASS', iteration,
    requests: eg ? [] : [receipt(slot, false)], eg_requests: eg ? [receipt(slot, true)] : [],
    owner_proofs: [], steps: [{id: 'expected_result', status: 'PASS', assertions: 1}], cleanup: []});
  const runs = [1, 2, 3].map(iteration => sample(iteration, 'a'));
  const status = results => coverageCounts(registry, results, binding).rows[0].status;
  return {sample, runs, status};
}

for (const eg of [true, false]) {
  const scope = eg ? 'EG' : 'AM', key = eg ? 'eg_requests' : 'requests';
  test(`${scope}: three qualified A runs plus an unproved B label cannot satisfy both ingresses`, () => {
    const f = fixture(eg);
    assert.notEqual(f.status(f.runs), 'PASS');
    const invalid = f.sample(4, 'b');
    invalid[key] = [eg ? {scope: 'eg', replica: 'b'} : {am: 'b'}];
    assert.notEqual(f.status([...f.runs, invalid]), 'PASS');
  });

  for (const [name, mutate] of [
    ['missing final process window', run => {delete run[key][0].serving_after;}],
    ['draining process', run => {run[key][0].serving.draining = true;}],
    ['changed process', run => {run[key][0].serving_after.container_id = 'changed';}],
    ['failed required step', run => {run.steps[0].status = 'FAIL';}],
    ['missing step assertions', run => {run.steps[0].assertions = 0;}],
    ['failed cleanup', run => {run.cleanup.push({status: 'FAIL'});}],
    ['unconfirmed parent disposal', run => {run.requires_parent_cleanup = true;}],
    ['unknown phase', run => {run.phase = 'unrecognized';}],
  ]) {
    test(`${scope}: ${name} on B cannot supplement qualified A runs`, () => {
      const f = fixture(eg), invalid = f.sample(4, 'b'); mutate(invalid);
      assert.notEqual(f.status([...f.runs, invalid]), 'PASS');
    });
  }

  test(`${scope}: a qualified B run counts without discarding incomplete attempts`, () => {
    const f = fixture(eg), validB = f.sample(4, 'b');
    assert.equal(f.status([...f.runs, validB]), 'PASS');
    const invalid = f.sample(5, 'b'); invalid[key] = [eg ? {scope: 'eg', replica: 'b'} : {am: 'b'}];
    const results = [...f.runs, validB, invalid], before = structuredClone(results);
    assert.equal(f.status(results), 'PASS');
    assert.deepEqual(results, before);
    assert.equal(f.status([...results, {...invalid, status: 'FAIL'}]), 'FAIL');
  });
}

for (const field of ['source', 'image']) {
  test(`EG: a structurally valid B receipt for the wrong selected ${field} cannot supply the peer`, () => {
    const f = fixture(true), wrong = f.sample(4, 'b');
    for (const snapshot of [wrong.eg_requests[0].serving, wrong.eg_requests[0].serving_after]) {
      snapshot[field] = field === 'source' ? 'f'.repeat(40) : 'other@sha256:' + 'f'.repeat(64);
    }
    assert.equal(validEgReceipt(wrong.eg_requests[0]), true);
    assert.notEqual(f.status([...f.runs, wrong]), 'PASS');
  });
}

// These are evidence-schema tests, not actual Telegram/Discord/WhatsApp calls.
// No fabricated artifact below is submitted as connected acceptance evidence.
for (const platform of ['telegram', 'discord', 'whatsapp']) {
  const event = {event_sha256: 'a'.repeat(64), conversation_sha256: 'b'.repeat(64), response_sha256: 'c'.repeat(64)};
  const proof = {schema: 'multi-am-external-platform-v1', kind: 'actual_platform', platform, synthetic: false, ...event,
    delivery: {platform, observed: true, artifact_sha256: 'd'.repeat(64)},
    sender: {platform, authenticated: true, independent: true, artifact_sha256: 'e'.repeat(64)}};
  test(`${platform}: actual sender, distinct artifacts and all three event identities are required`, () => {
    assert.equal(validPlatformProof(proof, platform, event), true);
    for (const mutate of [
      value => {value.synthetic = true;}, value => {value.kind = 'local_delivery';},
      value => {value.platform = 'other';}, value => {value.delivery.platform = 'other';},
      value => {value.sender.platform = 'other';}, value => {value.sender.authenticated = false;},
      value => {value.sender.independent = false;}, value => {value.delivery.observed = false;},
      value => {value.sender.artifact_sha256 = value.delivery.artifact_sha256;},
      value => {delete value.sender.artifact_sha256;}, value => {value.delivery.artifact_sha256 = 'not-a-hash';},
      ...['event_sha256', 'conversation_sha256', 'response_sha256'].map(key => value => {value[key] = 'f'.repeat(64);}),
    ]) {
      const invalid = structuredClone(proof); mutate(invalid);
      assert.equal(validPlatformProof(invalid, platform, event), false);
    }
    const id = {'telegram': 'C275:EG-032', discord: 'C275:EG-033', whatsapp: 'C275:EG-034'}[platform];
    const result = {platform_events: [event], external_platform_proofs: [proof], requests: [receipt('a', false)],
      owner_proofs: [{verdict: 'PASS', owner: 'b', ingress: 'a', owner_epoch: 1}]};
    assert.equal(platformComplete(id, result), true);
    assert.equal(platformComplete(id, {...result, platform_events: [event, {...event, event_sha256: 'f'.repeat(64)}]}), false);
    assert.equal(platformComplete(id, {...result, external_platform_proofs: []}), false);
    assert.equal(platformComplete(id, {...result, requests: []}), false);
    assert.equal(platformComplete(id, {...result, owner_proofs: []}), false);
  });
}
