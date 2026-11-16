import test from 'node:test';
import assert from 'node:assert/strict';
import {spendObservation} from './spend-api.mjs';
import {cloudObservationModel} from './cloud-observation-model.mjs';
import {cloudTime, validateObservationSpend} from './cloud-observation-data.mjs';

const run = h => spendObservation.run(h.ctx);
test('an invalid observation cannot change non-cloud accounting behind a refusal', async () => {
  const h = cloudObservationModel({changedInvalid: true});
  await assert.rejects(run(h), /admin_cloud_durable_changed/); await h.cleanup();
});
test('a successful-looking cloud observation must retain valid recorded metadata', async () => {
  const h = cloudObservationModel({malformedCloud: true});
  await assert.rejects(run(h), /admin_cloud_spend_shape/); await h.cleanup();
});
test('a completed cloud observation case restores the real parent writer gate', async () => {
  const h = cloudObservationModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.writes(), true); assert.deepEqual(h.modes, [false, true]);
});

const posts = h => h.calls.filter(c => c.method === 'POST');
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');
const failed = (h, code) => assert.rejects(run(h), e => messages(e).includes(code));
const observations = (h, label) => h.observations.filter(o => o.label === label).map(o => o.value);

for (const am of ['a', 'b']) {
  test('one original cloud write and both-AM typed refusals retain all response evidence from ' + am, async () => {
    const h = cloudObservationModel({am, committedDrift: true}); await run(h);
    const mutations = posts(h); assert.equal(mutations.length, 21);
    assert.equal(mutations.filter(p => p.allowed[0] === 200).length, 1);
    assert.equal(mutations[0].am, am); assert.equal(mutations[0].actor, 'admin');
    assert.deepEqual(mutations[0].body, {source: 'cloud', units: 10, observed_through: h.f.observedThrough});
    assert.equal(mutations.filter(p => p.allowed[0] === 422).length, 16);
    for (const status of [422, 503, 403]) {
      const selected = mutations.filter(p => p.allowed[0] === status);
      assert.deepEqual([...new Set(selected.map(p => p.am))], ['a', 'b']);
      assert.ok(selected.every(p => p.actor === (status === 403 ? 'user' : 'admin')));
    }
    assert.ok(mutations.slice(-4).every(p => p.body.units === 11));
    assert.equal(observations(h, 'admin_cloud_intent').length, 21);
    const responses = h.observations.filter(o => o.value.response);
    assert.equal(responses.length, h.calls.length);
    for (const {value: {am: actual, response}} of responses) {
      assert.deepEqual(response.timing, {am: actual, request_id: response.timing.request_id,
        serving: {pod_uid: 'model-' + actual}, serving_after: {pod_uid: 'model-' + actual},
        started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
    }
    const refusals = observations(h, 'admin_cloud_after_refusal'); assert.equal(refusals.length, 40);
    assert.ok(new Set(refusals.map(o => o.response.body.committed)).size > 1);
    const count = h.calls.length; assert.deepEqual(await h.cleanup(), []);
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, count); assert.deepEqual(h.modes, [false, true]);
  });
}
for (const option of ['emptyLedger', 'previousCloud']) {
  test('valid observation handles ' + option + ' without changing other accounting', async () => {
    const h = cloudObservationModel({[option]: true}); await run(h); assert.deepEqual(await h.cleanup(), []);
    assert.equal(h.state().ledger.cloud.units, h.f.units); assert.equal(h.state().ledger.spent, 0);
    assert.equal(h.state().ledger.reserved, 0); assert.equal(h.state().ledger.adjustments, 0);
  });
}
test('maximum safe units keep disabled-writer and forbidden variants within valid integer range', async () => {
  const h = cloudObservationModel({units: Number.MAX_SAFE_INTEGER}); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.ok(posts(h).slice(-4).every(p => p.body.units === Number.MAX_SAFE_INTEGER - 1));
});

for (const [option, code] of [['initialWrites', 'admin_cloud_writable_metering_fixture_missing'],
  ['disabled', 'admin_cloud_writable_metering_fixture_missing'], ['splitSettings', 'admin_cloud_initial_peer_mismatch'],
  ['splitLedger', 'admin_cloud_initial_peer_mismatch']]) {
  test(option + ' is refused before any observation or mode change', async () => {
    const h = cloudObservationModel({[option]: option !== 'initialWrites'}); await failed(h, code);
    assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0); assert.deepEqual(h.modes, []);
  });
}
test('same-instant fixture would only repeat an existing observation and is blocked', async () => {
  const h = cloudObservationModel({previousCloud: true, existingInstant: true});
  await failed(h, 'admin_cloud_fresh_observation_required');
  assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0);
});
for (const value of [0, -1, 1.5, NaN, Number.MAX_SAFE_INTEGER + 1]) {
  test('invalid fixture accounting units are blocked: ' + value, async () => {
    const h = cloudObservationModel(); h.f.units = value;
    await failed(h, 'admin_cloud_observation_fixture_missing');
    assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0);
  });
}
for (const value of ['2026-11-17T00:00:00.000Z', '2026-02-30T00:00:00Z', 'yesterday', '2100-01-01T00:00:00Z']) {
  test('unsupported fixture timestamp is blocked before dispatch: ' + value, async () => {
    const h = cloudObservationModel(); h.f.observedThrough = value;
    await failed(h, 'admin_cloud_observation_fixture_missing');
    assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0);
  });
}
test('out-of-period fixture timestamp is blocked before dispatch', async () => {
  const h = cloudObservationModel(); h.f.observedThrough = '2000-01-01T00:00:00Z';
  await failed(h, 'admin_cloud_period_fixture_missing'); assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0);
});
for (const field of ['exclusiveSettings', 'setWritesAvailable']) {
  test('authentic fixture requires ' + field, async () => {
    const h = cloudObservationModel(); delete h.f[field]; await failed(h, 'admin_cloud_observation_fixture_missing');
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, 0);
  });
}
for (const [option, count] of [['changedInvalid', 2], ['changedGate', 18], ['changedRole', 20]]) {
  test(option + ' stops further dispatch and remains failed through final reads', async () => {
    const h = cloudObservationModel({[option]: true}); await failed(h, 'admin_cloud_durable_changed');
    const errors = await h.cleanup(); assert.equal(errors.length, 1);
    assert.equal(errors[0].errors.length, 2); assert.match(messages(errors[0]), /admin_cloud_durable_changed/);
    assert.equal(posts(h).length, count); assert.equal(h.writes(), true);
    assert.deepEqual(observations(h, 'admin_cloud_final_spend').map(o => o.am), ['a', 'b']);
  });
}
for (const [option, count] of [['wrongInvalidCode', 2], ['wrongGateCode', 18], ['wrongRoleCode', 20],
  ['leakedRefusal', 2], ['refusalSuccess', 2]]) {
  test(option + ' never counts as a typed safe refusal', async () => {
    const h = cloudObservationModel({[option]: true});
    await failed(h, option === 'leakedRefusal' ? 'admin_cloud_refusal_exposed_accounting'
      : option === 'refusalSuccess' ? 'status_200' : 'admin_cloud_wrong_refusal');
    assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, count);
  });
}
test('a valid-looking acknowledgement cannot change unrelated ledger fields', async () => {
  const h = cloudObservationModel({changedSuccessful: true}); await failed(h, 'admin_cloud_success_changed_unrelated_state');
  const errors = await h.cleanup(); assert.match(messages(errors[0]), /admin_cloud_observation_outcome_unproven/);
  assert.equal(posts(h).length, 1); assert.deepEqual(h.modes, []);
});
for (const option of ['splitGate', 'ignoreMode', 'changedModeSettings']) {
  test(option + ' prevents unavailable-mode POSTs and still restores once', async () => {
    const h = cloudObservationModel({[option]: true});
    await failed(h, option === 'changedModeSettings' ? 'admin_cloud_settings_changed' : 'admin_cloud_writer_gate');
    const errors = await h.cleanup(); assert.equal(errors.length, option === 'changedModeSettings' ? 1 : 0);
    assert.equal(posts(h).length, 17); assert.deepEqual(h.modes, [false, true]);
  });
}
for (const option of ['unknownSave', 'unknownSaveBefore']) {
  test(option + ' retains both-AM observations and uncertainty without replay or ledger repair', async () => {
    const h = cloudObservationModel({[option]: true}); await failed(h, 'save_unknown');
    const errors = await h.cleanup(); assert.match(messages(errors[0]), /admin_cloud_observation_outcome_unproven/);
    assert.equal(posts(h).length, 1); assert.deepEqual(h.modes, []);
    assert.deepEqual(observations(h, 'admin_cloud_final_spend').map(o => o.am), ['a', 'b']);
    const count = h.calls.length; assert.equal((await h.cleanup())[0], errors[0]); assert.equal(h.calls.length, count);
  });
}
for (const [option, code, count] of [['unknownInvalid', 'invalid_unknown', 2],
  ['unknownGate', 'gate_unknown', 18], ['unknownRole', 'role_unknown', 20]]) {
  test(option + ' stops dispatch and original unknown response is never replayed', async () => {
    const h = cloudObservationModel({[option]: true}); await failed(h, code);
    assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, count);
    const countBefore = h.calls.length; await h.cleanup(); assert.equal(h.calls.length, countBefore);
  });
}
test('lost disable acknowledgement retains restoration intent', async () => {
  const h = cloudObservationModel({unknownMode: true}); await failed(h, 'mode_unknown');
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.modes, [false, true]); assert.equal(posts(h).length, 17);
});
test('lost restore acknowledgement remains sticky even with correct readbacks', async () => {
  const h = cloudObservationModel({unknownRestore: true}); await failed(h, 'admin_cloud_finality');
  const errors = await h.cleanup(); assert.match(messages(errors[0]), /restore_unknown/);
  assert.equal(h.writes(), true); const count = h.calls.length;
  assert.equal((await h.cleanup())[0], errors[0]); assert.equal(h.calls.length, count); assert.deepEqual(h.modes, [false, true]);
});
test('restoration and both failed A reads retain independent errors and both B responses', async () => {
  const h = cloudObservationModel({unknownRestore: true, finalReadFailure: true}); await failed(h, 'admin_cloud_finality');
  const errors = await h.cleanup(); assert.equal(errors[0].errors.length, 3);
  assert.match(messages(errors[0]), /restore_unknown/); assert.match(messages(errors[0]), /final_read_failed/);
  assert.deepEqual(observations(h, 'admin_cloud_final_settings').map(o => o.am), ['b']);
  assert.deepEqual(observations(h, 'admin_cloud_final_spend').map(o => o.am), ['b']);
});
for (const interruptedStep of ['step_1', 'step_2', 'expected_result']) {
  test('interruption before ' + interruptedStep + ' finalizes outside active steps', async () => {
    const h = cloudObservationModel({interruptedStep}); await failed(h, 'step_interrupted');
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.writes(), true);
    assert.deepEqual(h.modes, interruptedStep === 'expected_result' ? [false, true] : []);
  });
}
test('parent disposal is attempted and its failure preserved after contributor finality failure', async () => {
  const h = cloudObservationModel({unknownRestore: true}); let disposed = 0;
  h.f.cleanup = async () => {disposed++; throw Error('disposal_failed');};
  await failed(h, 'admin_cloud_finality'); const errors = await h.cleanup();
  assert.equal(disposed, 1); assert.equal(errors.length, 2); assert.match(messages(errors[1]), /disposal_failed/);
});

test('timestamp validation follows the exact whole-second UTC API parser', () => {
  assert.equal(cloudTime('1970-01-01T00:00:00Z'), 0);
  assert.equal(cloudTime('2024-02-29T00:00:00Z'), Date.parse('2024-02-29T00:00:00Z'));
  for (const stamp of [null, '', '1969-12-31T23:59:59Z', '2026-02-30T00:00:00Z', '2026-01-01T24:00:00Z',
    '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00+00:00', '2026-01-01']) assert.ok(Number.isNaN(cloudTime(stamp)));
});
test('malformed accounting and cloud metadata never qualify as a saved ledger', async () => {
  const h = cloudObservationModel(); await run(h); await h.cleanup(); const good = h.state();
  assert.equal(validateObservationSpend(good), true);
  const mutations = [
    v => {delete v.settings_revision;}, v => {v.settings_revision = -1;}, v => {v.levels = null;},
    v => {v.levels.stop = 1.5;}, v => {v.committed = Number.MAX_SAFE_INTEGER + 1;}, v => {v.ledger = null;},
    v => {v.ledger.spent = -1;}, v => {v.ledger.reserved = 1.5;}, v => {v.ledger.period_end = v.ledger.period_start;},
    v => {v.ledger.evaluated_at = 'bad';}, v => {v.ledger.closures = [null];},
    v => {v.ledger.cloud = null;}, v => {v.ledger.cloud.units = -1;}, v => {v.ledger.cloud.settings_revision = '7';},
    v => {v.ledger.cloud.recorded_at = v.ledger.period_start;}, v => {v.ledger.cloud.observed_through = 'yesterday';},
    v => {v.ledger.cloud.observed_through = '2000-01-01T00:00:00Z';},
    v => {v.ledger.closures = [{reason: 'unknown', since: good.ledger.period_start, settings_revision: 7, committed_at_entry: 1}];},
  ];
  for (const mutate of mutations) {const value = structuredClone(good); mutate(value); assert.equal(validateObservationSpend(value), false);}
  for (const value of [null, undefined, {}, []]) assert.equal(validateObservationSpend(value), false);
  assert.equal(validateObservationSpend({metering_enabled: false, settings_revision: 7}), true);
  assert.equal(validateObservationSpend({metering_enabled: false, settings_revision: 7, ledger: {}}), false);
  good.ledger.evaluated_at = good.ledger.cloud.recorded_at;
  good.ledger.closures = [{reason: 'spend_warning', since: good.ledger.cloud.recorded_at, settings_revision: 7, committed_at_entry: 10}];
  assert.equal(validateObservationSpend(good), true);
});
