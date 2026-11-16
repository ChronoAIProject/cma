import test from 'node:test';
import assert from 'node:assert/strict';
import {automaticPause} from './settings-lifecycle.mjs';
import {automaticModel} from './automatic-pause-model.mjs';

test('default case budget blocks before taking a fixture or changing settings', async () => {
  const h = automaticModel({budget: 60000});
  await assert.rejects(automaticPause.run(h.ctx), /admin_auto_pause_budget/);
  assert.equal(h.setupCount, 0); assert.equal(h.calls.length, 0);
});

test('missing live busy-work evidence blocks before settings/browser requests', async () => {
  const h = automaticModel(); delete h.f.readBusyWork;
  await assert.rejects(automaticPause.run(h.ctx), /admin_auto_pause_cohort/);
  assert.equal(h.calls.length, 0); await h.cleanup();
});

import {automaticPauseScenario} from './automatic-pause.mjs';
import {waitAutomaticGrace, automaticPoll, realClock} from './automatic-pause-time.mjs';
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');
const run = h => automaticPauseScenario(h.ctx, h.clock);

test('actual monotonic grace, exact UI saves and both-AM lifecycle precede restoration', async () => {
  const h = automaticModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  assert.ok(!h.actions.some(a => a.op === 'fixture_oracle'));
  assert.equal(h.actions.filter(a => a.op === 'sleep').reduce((sum, a) => sum + a.ms, 0), 60000);
  const grace = h.observations.find(o => o.label === 'admin_auto_pause_grace').value;
  assert.equal(grace.elapsed_ms, 60000); assert.equal(grace.revision, 4);
  assert.equal(grace.grace_started_at, '2026-11-17T00:00:00.000Z');
  assert.deepEqual(h.actions.filter(a => a.op === 'save').map(a => a.settings.inactivity.pause_after_seconds), [86400, 60, 60]);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
  const final = h.observations.filter(o => o.label === 'admin_auto_pause_final_lifecycle');
  assert.deepEqual(final.map(o => o.value.am), ['a', 'b']);
  assert.ok(final.every(o => o.value.states[0].status === 'paused'));
  const receipt = h.observations.filter(o => o.label === 'admin_auto_pause_browser').at(-1).value.requests.at(-1);
  for (const key of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(receipt[key]);
});

for (const [option, code] of [
  ['wrongId', 'admin_idle_detail_identity'], ['wrongPolicy', 'admin_auto_pause_initial_policy'],
  ['initialOperation', 'admin_auto_pause_initial_lifecycle'], ['malformedWork', 'admin_auto_pause_busy_shape'],
  ['wrongRefusal', 'admin_auto_pause_invalid_refusal'], ['failedSave', 'admin_auto_pause_save_response'],
  ['badRevision', 'admin_auto_pause_saved_policy'], ['wrongBody', 'admin_auto_pause_save_body'],
  ['stalePeer', 'admin_auto_pause_peer_policy'], ['peerGrace', 'admin_auto_pause_peer_policy'],
  ['badGrace', 'admin_auto_pause_server_grace'], ['unjoinedSave', 'admin_auto_pause_ui_ingress'],
  ['wrongServing', 'admin_auto_pause_ui_ingress'], ['bearerOnly', 'admin_auto_pause_cookie_session'],
  ['lateWrite', 'admin_auto_pause_extra_write'], ['protectedStatus', 'admin_auto_pause_protected_status'],
  ['protectedEvent', 'admin_auto_pause_protected_changed'], ['protectedEffect', 'admin_auto_pause_protected_changed'],
  ['pendingOperation', 'admin_auto_pause_incomplete_lifecycle'], ['stalled', 'admin_auto_pause_busy_interrupted'],
  ['restarted', 'admin_auto_pause_busy_interrupted'], ['abortWait', 'test_abort'], ['stillActive', 'admin_auto_pause_timeout'],
]) test('unproven automatic pause fails: ' + option, async () => {
  const h = automaticModel({[option]: true});
  await assert.rejects(run(h), e => messages(e).includes(code));
  await h.cleanup();
  assert.ok(h.actions.filter(a => a.op === 'save').length <= 3);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});

for (const changedWork of ['sandbox_id', 'agent_id', 'turn_sequence', 'runtime_id']) {
  test('busy progress retains the same original identity: ' + changedWork, async () => {
    const h = automaticModel({changedWork});
    await assert.rejects(run(h), e => /admin_auto_pause_busy_identity|admin_auto_pause_busy_shape/.test(messages(e)));
    await h.cleanup();
  });
}

for (const invalid of ['duplicate', 'missing_kind', 'bad_id', 'missing_lease']) {
  test('malformed cohort is rejected before settings reads: ' + invalid, async () => {
    const h = automaticModel();
    if (invalid === 'duplicate') h.f.cohort[1].id = h.f.cohort[0].id;
    if (invalid === 'missing_kind') h.f.cohort.pop();
    if (invalid === 'bad_id') h.f.cohort[0].id = 'bad/id';
    if (invalid === 'missing_lease') delete h.f.exclusiveSettings;
    await assert.rejects(run(h)); assert.equal(h.calls.length, 0);
    assert.deepEqual(await h.cleanup(), []);
  });
}

test('unknown UI acknowledgement restores observed owned policy once after drain', async () => {
  const h = automaticModel({unknownSave: true});
  await assert.rejects(run(h), /save_unknown/); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  assert.equal(h.actions.filter(a => a.op === 'save').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});

test('restoration acknowledgement is never retried or converted to cleanup success', async () => {
  const h = automaticModel({restoreFailure: true});
  await assert.rejects(run(h), e => messages(e).includes('restore_unknown'));
  assert.ok((await h.cleanup()).some(e => messages(e).includes('restore_unknown')));
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
});

test('close, capture, each-AM lifecycle, work and restoration failures remain independent in cleanup', async () => {
  const h = automaticModel({closeFailure: true, evidenceFailure: true, snapshotFailure: true,
    workReadFailure: true, restoreFailure: true});
  await assert.rejects(run(h), e => messages(e).includes('close_failed'));
  const errors = await h.cleanup();
  for (const code of ['close_failed', 'evidence_failed', 'snapshot_failed', 'busy_read_failed', 'restore_unknown'])
    assert.ok(errors.some(e => messages(e).includes(code)), code);
  assert.equal(h.actions.filter(a => a.op === 'close').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(!errors.some(e => messages(e).includes('assertion_outside_step')));
});

test('second selected AM joins actual saves and both peer policies', async () => {
  const h = automaticModel(); h.ctx.am = 'b';
  await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.ok(h.receipts.filter(r => r.method === 'PUT').every(r => r.am === 'b' && r.serving_am === 'b'));
});

const stored = {revision: 9, settings: {inactivity: {auto_pause_enabled: true,
  pause_after_seconds: 60, grace_started_at: '2026-11-17T00:00:00Z'}}};
for (const policy of [null, {auto_pause_enabled: false}, {auto_pause_enabled: true, pause_after_seconds: 59},
  {...stored.settings.inactivity, grace_started_at: 'invalid'}]) {
  test('wait rejects absent or invalid server grace: ' + JSON.stringify(policy), async () => {
    const h = automaticModel();
    await assert.rejects(waitAutomaticGrace(h.ctx, {settings: {inactivity: policy}}, h.clock), /admin_auto_pause_server_grace/);
    assert.equal(h.actions.length, 0);
  });
}

test('already aborted wait issues no read or timer', async () => {
  const h = automaticModel(); h.controller.abort(Error('already_aborted'));
  await assert.rejects(waitAutomaticGrace(h.ctx, stored, h.clock), /already_aborted/);
  assert.equal(h.actions.length, 0);
});

test('a forward wall clock jump never substitutes for monotonic grace', async () => {
  const h = automaticModel(); let wall = 0;
  const clock = {...h.clock, wall: () => Date.parse('2026-11-17T00:00:00Z') + (++wall * 86400000)};
  await waitAutomaticGrace(h.ctx, stored, clock);
  assert.equal(h.actions.filter(a => a.op === 'sleep').reduce((sum, a) => sum + a.ms, 0), 60000);
});

test('remaining budget loss and a backward monotonic clock cannot pass', async () => {
  const h = automaticModel(); let budgets = 0;
  h.ctx.remainingMs = () => ++budgets === 1 ? 180000 : 500;
  await assert.rejects(waitAutomaticGrace(h.ctx, stored, h.clock), /admin_auto_pause_wait_budget/);
  let ticks = 0;
  const other = automaticModel();
  await assert.rejects(waitAutomaticGrace(other.ctx, stored, {...other.clock, now: () => ticks++ ? -1 : 0}),
    /admin_auto_pause_clock/);
});

test('poll preserves a bounded deadline and abort instead of another request', async () => {
  const h = automaticModel(); let reads = 0;
  h.ctx.remainingMs = () => 15500;
  await assert.rejects(automaticPoll(h.ctx, async () => {reads++; return false;}, Boolean, h.clock), /admin_auto_pause_timeout/);
  assert.equal(reads, 2); assert.equal(h.actions[0].ms, 500);
  h.controller.abort(Error('poll_aborted'));
  await assert.rejects(automaticPoll(h.ctx, async () => {reads++;}, Boolean, h.clock), /poll_aborted/);
  assert.equal(reads, 2);
});

test('production clock uses the abortable standard timer', async () => {
  assert.ok(Number.isFinite(realClock.now())); assert.ok(Number.isFinite(realClock.wall()));
  const controller = new AbortController(); controller.abort();
  await assert.rejects(realClock.sleep(1, controller.signal), {name: 'AbortError'});
});

test('an erroneously accepted invalid threshold fails and retains restoration custody', async () => {
  const h = automaticModel({invalidAccepted: true});
  await assert.rejects(run(h), /status_200/);
  assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  assert.equal(h.calls.filter(c => c.method === 'PATCH').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});

for (const remaining of [90000, Infinity, NaN, undefined]) {
  test('missing or invalid remaining budget blocks without setup: ' + remaining, async () => {
    const h = automaticModel(); h.ctx.remainingMs = remaining === undefined ? undefined : () => remaining;
    await assert.rejects(run(h), /admin_auto_pause_budget/); assert.equal(h.setupCount, 0);
  });
}
