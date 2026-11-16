import test from 'node:test';
import assert from 'node:assert/strict';
import {idlePause} from './enforcement.mjs';
import {pauseModel} from './idle-pause-model.mjs';
import {pauseIdleScenario} from './idle-pause.mjs';

const messages = error => [error.message, ...(error.errors ?? []).map(messages)].join(' ');

test('confirmed pause joins UI receipt, drains, then proves both-AM completion and the same busy turn', async () => {
  const h = pauseModel(); await idlePause.run(h.ctx);
  assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.browserCalls.filter(c => c.operation === 'click').length, 1);
  assert.equal(h.browserCalls.filter(c => c.operation === 'accept').length, 1);
  assert.ok(h.busyReads >= 2);
  assert.ok(h.browserCalls.some(c => c.operation === 'busy_read' && c.closed));
  assert.ok(h.browserCalls.some(c => c.operation === 'evidence' && c.closed));
  const ack = h.observations.find(o => o.label === 'admin_pause_ack').value;
  assert.equal(ack.status, 200); assert.equal(ack.request_id, 'browser-pause');
  assert.deepEqual(ack.body, {scheduled: 1, already_satisfied: 0, skipped: 2});
  const snapshots = h.observations.filter(o => o.label === 'admin_pause_final_lifecycle');
  assert.equal(snapshots.length, 2);
  assert.deepEqual(new Set(snapshots.map(o => o.value.am)), new Set(['a', 'b']));
  assert.ok(h.calls.filter(c => c.method === 'POST').every(c => c.body.confirmed === false));
  const receipt = h.observations.filter(o => o.label === 'admin_browser_evidence').at(-1).value.requests.at(-1);
  for (const key of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(receipt[key]);
});

for (const [option, code] of [
  ['failedAck', 'admin_pause_ack_response'], ['unjoinedResponse', 'admin_pause_browser_ingress'],
  ['wrongServing', 'admin_pause_browser_ingress'], ['wrongBody', 'admin_pause_confirmation_body'],
  ['wrongSkipped', 'admin_pause_scheduled_wrong_cohort'], ['alreadyScheduled', 'admin_pause_scheduled_wrong_cohort'],
  ['bearerOnly', 'admin_pause_cookie_session_required'], ['wrongRefusal', 'admin_pause_unconfirmed_refusal'],
  ['unconfirmedEffect', 'admin_pause_unconfirmed_effect'], ['protectedEffect', 'admin_pause_protected_lifecycle_changed'],
  ['lateProtectedEffect', 'admin_pause_protected_lifecycle_changed'], ['wrongDetail', 'admin_idle_detail_identity'],
  ['initialPause', 'admin_pause_initial_lifecycle'], ['wrongAlwaysOn', 'admin_pause_initial_policy'],
  ['busyAlwaysOn', 'admin_pause_initial_policy'], ['pendingIdleOperation', 'admin_pause_incomplete_lifecycle'],
  ['wrongDialog', 'admin_pause_confirmation_dialog'], ['wrongDialogType', 'admin_pause_confirmation_dialog'],
  ['earlyWrite', 'admin_pause_browser_extra_write'], ['malformedInitial', 'admin_pause_busy_shape'],
]) {
  test('unproven or unsafe confirmed pause fails: ' + option, async () => {
    const h = pauseModel({[option]: true});
    await assert.rejects(idlePause.run(h.ctx), e => messages(e).includes(code));
    await h.cleanup();
    assert.ok(h.browserCalls.filter(c => c.operation === 'click').length <= 1);
    if (option.startsWith('wrongDialog')) {
      assert.equal(h.browserCalls.filter(c => c.operation === 'accept').length, 0);
      assert.equal(h.browserCalls.filter(c => c.operation === 'dismiss').length, 1);
    }
  });
}

for (const changedWork of ['sandbox_id', 'agent_id', 'turn_sequence', 'runtime_id']) {
  test('higher progress for a different busy identity cannot pass: ' + changedWork, async () => {
    const h = pauseModel({changedWork});
    await assert.rejects(idlePause.run(h.ctx), e => messages(e).includes('admin_pause_busy_identity'));
    await h.cleanup();
  });
}
for (const option of ['stalled', 'restarted']) {
  test('busy turn must progress without restart: ' + option, async () => {
    const h = pauseModel({[option]: true});
    await assert.rejects(idlePause.run(h.ctx), e => messages(e).includes('admin_pause_busy_turn_interrupted'));
    await h.cleanup();
  });
}
for (const lateMutation of ['POST', 'PUT', 'DELETE']) {
  test('a late browser mutation fails after drain: ' + lateMutation, async () => {
    const h = pauseModel({lateMutation});
    await assert.rejects(idlePause.run(h.ctx), e => messages(e).includes('admin_pause_browser_extra_write'));
    const failures = await h.cleanup();
    assert.ok(failures.some(e => messages(e).includes('admin_pause_browser_extra_write')));
    assert.equal(h.browserCalls.filter(c => c.operation === 'click').length, 1);
  });
}

test('close, evidence, durable-state and busy-read errors remain independent', async () => {
  const h = pauseModel({closeFailure: true, evidenceFailure: true, lateProtectedEffect: true, workReadFailure: true});
  await assert.rejects(idlePause.run(h.ctx));
  const failures = await h.cleanup();
  for (const code of ['pause_close_failed', 'pause_evidence_failed', 'admin_pause_protected_lifecycle_changed', 'busy_read_failed'])
    assert.ok(failures.some(e => messages(e).includes(code)), code);
  assert.equal(h.browserCalls.filter(c => c.operation === 'close').length, 1);
  assert.equal(h.browserCalls.filter(c => c.operation === 'click').length, 1);
});

for (const invalid of ['duplicate', 'work_reader', 'invalid_id']) {
  test('invalid parent contract blocks before requests: ' + invalid, async () => {
    const h = pauseModel();
    if (invalid === 'duplicate') h.cohort[1].id = h.cohort[0].id;
    if (invalid === 'work_reader') delete h.f.readBusyWork;
    if (invalid === 'invalid_id') h.cohort[0].id = 'bad/id';
    await assert.rejects(idlePause.run(h.ctx));
    assert.equal(h.calls.length, 0);
    await h.cleanup();
  });
}

for (const ack of [{scheduled: -1}, {scheduled: 0}, {skipped: 1.5}, {already_satisfied: null}]) {
  test('invalid or incomplete scheduling counts cannot qualify: ' + JSON.stringify(ack), async () => {
    const h = pauseModel({ack});
    await assert.rejects(idlePause.run(h.ctx), /admin_pause_ack_shape|admin_pause_scheduled_wrong_cohort/);
    await h.cleanup();
  });
}

test('an accepted schedule still fails when both-AM lifecycle completion is absent', async () => {
  const h = pauseModel({stillPausing: true}); let polls = 0;
  const boundedPoll = async (operation, complete, code) => {
    const states = await operation(); polls++;
    assert.equal(states.length, 6); assert.equal(complete(states), false);
    throw Error(code);
  };
  await assert.rejects(pauseIdleScenario(h.ctx, boundedPoll), /admin_pause_lifecycle_timeout/);
  assert.equal(polls, 1);
  const ack = h.observations.find(o => o.label === 'admin_pause_ack').value;
  assert.equal(ack.status, 200); assert.equal(ack.body.scheduled, 1);
  await h.cleanup();
  assert.equal(h.browserCalls.filter(c => c.operation === 'click').length, 1);
});

test('the second ingress uses its actual acknowledgement and opposite-AM refusal readback', async () => {
  const h = pauseModel(); h.ctx.am = 'b';
  await idlePause.run(h.ctx); assert.deepEqual(await h.cleanup(), []);
  const writes = h.receipts.filter(r => r.method === 'POST');
  assert.equal(writes.length, 1); assert.equal(writes[0].am, 'b'); assert.equal(writes[0].serving_am, 'b');
  const refusal = h.calls.findIndex(c => c.method === 'POST');
  assert.equal(h.calls[refusal + 1].am, 'a');
});
