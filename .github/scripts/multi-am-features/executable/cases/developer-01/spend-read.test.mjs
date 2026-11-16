import test from 'node:test';
import assert from 'node:assert/strict';
import {spendRead} from './spend-api.mjs';
import {spendReadModel} from './spend-read-model.mjs';
import {verifySpendRead, verifySpendRefusal} from './spend-read-data.mjs';

const run = h => spendRead.run(h.ctx);
test('missing cloud observation cannot fabricate committed zero', async () => {
  const h = spendReadModel({fabricatedZero: true}); await assert.rejects(run(h)); await h.cleanup();
});
test('dependency failure cannot leak private ledger values', async () => {
  const h = spendReadModel({leakedUnavailable: true}); await assert.rejects(run(h)); await h.cleanup();
});
test('the original real fixture mode is restored after all read variants', async () => {
  const h = spendReadModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.mode(), 'enabled'); assert.equal(h.restoreCalls(), 1);
});

const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');
const failed = (h, code) => assert.rejects(run(h), e => messages(e).includes(code));
const observations = (h, label) => h.observations.filter(o => o.label === label).map(o => o.value);

for (const am of ['a', 'b']) {
  test('four real mode selections retain both-AM reads and authorization receipts from ' + am, async () => {
    const h = spendReadModel({am}); await run(h);
    assert.equal(h.calls.length, 42); assert.ok(h.calls.every(c => c.method === 'GET' && c.body === undefined));
    assert.deepEqual(h.modes, ['enabled', 'disabled', 'dependency_unavailable', 'enabled_without_cloud']);
    for (const mode of h.modes) {
      const denied = h.calls.filter(c => c.actor === 'user' && c.mode === mode);
      assert.deepEqual(denied.map(c => c.am), ['a', 'b']); assert.ok(denied.every(c => c.allowed[0] === 403));
      const reads = h.calls.filter(c => c.actor === 'admin' && c.mode === mode && c.path.endsWith('/spend'));
      assert.deepEqual([...new Set(reads.map(c => c.am))], ['a', 'b']);
      assert.ok(reads.every(c => c.allowed[0] === (mode === 'dependency_unavailable' ? 503 : 200)));
    }
    const responses = h.observations.filter(o => o.value.response); assert.equal(responses.length, h.calls.length);
    for (const {value: {am: actual, response}} of responses) {
      assert.deepEqual(response.timing, {am: actual, request_id: response.timing.request_id,
        serving: {pod_uid: 'model-' + actual}, serving_after: {pod_uid: 'model-' + actual},
        started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
    }
    assert.equal(observations(h, 'admin_spend_enabled_spend')[0].response.body.committed, 15);
    assert.equal(observations(h, 'admin_spend_enabled_spend')[0].response.body.ledger.cloud.units, 5);
    assert.ok(observations(h, 'admin_spend_no_observation_spend').every(o => !Object.hasOwn(o.response.body, 'committed')));
    const count = h.calls.length; assert.deepEqual(await h.cleanup(), []); assert.deepEqual(await h.cleanup(), []);
    assert.equal(h.calls.length, count); assert.equal(h.restoreCalls(), 1);
  });
}
for (const initialMode of ['disabled', 'enabled_without_cloud']) {
  test('restoration preserves the actual initial mode ' + initialMode, async () => {
    const h = spendReadModel({initialMode}); await run(h); assert.deepEqual(await h.cleanup(), []);
    assert.equal(h.mode(), initialMode); assert.equal(h.restoreCalls(), 1);
  });
}
test('absent cloud configuration disables platform metering even when budgets stay enabled', async () => {
  const h = spendReadModel({withoutPolicy: true}); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.ok(observations(h, 'admin_spend_disabled_settings').every(o => o.response.body.settings.model_budget.enabled));
  assert.ok(observations(h, 'admin_spend_disabled_spend').every(o => o.response.body.metering_enabled === false));
});
test('a new settings revision after genuine restoration preserves policy and original accounting', async () => {
  const h = spendReadModel({restoredRevision: true}); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.ok(observations(h, 'admin_spend_final_settings').every(o => o.response.body.revision === 20));
  assert.ok(observations(h, 'admin_spend_final_spend').every(o => o.response.body.settings_revision === 20
    && o.response.body.ledger.settings_revision === 11));
});

for (const key of ['exclusiveSettings', 'selectMode', 'restoreMode']) {
  test('parent mode custody requires ' + key + ' before any read', async () => {
    const h = spendReadModel(); delete h.f[key]; await failed(h, 'admin_spend_modes_missing');
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, 0); assert.equal(h.restoreCalls(), 0);
  });
}
for (const [field, code] of [['expectedEnabled', 'admin_spend_enabled_inputs_missing'],
  ['expectedDisabled', 'admin_spend_disabled_inputs_missing'], ['expectedWithoutCloud', 'admin_spend_missing_cloud_inputs_missing']]) {
  test('missing concrete accounting inputs ' + field + ' cannot register a successful fixture', async () => {
    const h = spendReadModel(); delete h.f[field]; await failed(h, code);
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, 0); assert.deepEqual(h.modes, []);
  });
}
test('prepared disabled or missing-observation expected values cannot normalize fabricated zeros', async () => {
  for (const field of ['expectedDisabled', 'expectedWithoutCloud']) {
    const h = spendReadModel(); h.f[field].committed = 0;
    await assert.rejects(run(h)); assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, 0);
  }
});
test('positive metered cohort must include an actual cloud observation and committed value', async () => {
  for (const field of ['committed', 'ledger']) {
    const h = spendReadModel(); delete h.f.expectedEnabled[field]; await failed(h, 'admin_spend_enabled_inputs_missing');
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, 0);
  }
});
for (const [option, code] of [['splitState', 'admin_spend_mode_revision'], ['badLevelsPolicy', 'admin_spend_levels_mismatch']]) {
  test('initial ' + option + ' refuses before fixture mode changes', async () => {
    const h = spendReadModel({[option]: true}); await failed(h, code);
    assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.modes, []); assert.equal(h.restoreCalls(), 0);
  });
}
for (const [option, code] of [['fabricatedZero', 'admin_missing_cloud_fabricated_zero'],
  ['nullCloud', 'admin_spend_read_shape'], ['disabledValues', 'admin_spend_read_shape'],
  ['changedLedger', 'admin_spend_accounting_mismatch'], ['changedCommitted', 'admin_spend_accounting_mismatch']]) {
  test(option + ' is never a valid configured spend sample', async () => {
    const h = spendReadModel({[option]: true}); await failed(h, code);
    await h.cleanup(); assert.equal(h.mode(), 'enabled'); assert.equal(h.restoreCalls(), 1);
  });
}
for (const [option, code] of [['leakedUnavailable', 'admin_spend_refusal_exposed_accounting'],
  ['leakedUser', 'admin_spend_refusal_exposed_accounting'], ['wrongRoleCode', 'admin_spend_wrong_refusal'],
  ['wrongUnavailableCode', 'admin_spend_wrong_refusal'], ['allowUser', 'status_200'], ['availableDependency', 'status_200']]) {
  test(option + ' retains failure and still restores the original mode', async () => {
    const h = spendReadModel({[option]: true}); await failed(h, code);
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.mode(), 'enabled'); assert.equal(h.restoreCalls(), 1);
  });
}
test('a selector that never disables metering is detected without progressing to the fault mode', async () => {
  const h = spendReadModel({ignoreMode: 'disabled'}); await failed(h, 'admin_spend_accounting_mismatch');
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.modes, ['enabled', 'disabled']); assert.equal(h.restoreCalls(), 1);
});
for (const unknownMode of ['enabled', 'disabled', 'dependency_unavailable', 'enabled_without_cloud']) {
  test('lost acknowledgement selecting ' + unknownMode + ' triggers one restoration without replay', async () => {
    const h = spendReadModel({unknownMode}); await failed(h, 'mode_unknown');
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.modes.at(-1), unknownMode);
    assert.equal(h.modes.filter(m => m === unknownMode).length, 1); assert.equal(h.restoreCalls(), 1);
    assert.deepEqual(observations(h, 'admin_spend_final_spend').map(o => o.am), ['a', 'b']);
    const count = h.calls.length; await h.cleanup(); assert.equal(h.calls.length, count);
  });
}
test('lost restoration acknowledgement stays failed even after valid independent readbacks', async () => {
  const h = spendReadModel({unknownRestore: true}); await failed(h, 'admin_spend_read_finality');
  const errors = await h.cleanup(); assert.match(messages(errors[0]), /restore_unknown/);
  assert.deepEqual(observations(h, 'admin_spend_final_spend').map(o => o.am), ['a', 'b']);
  const count = h.calls.length; assert.equal((await h.cleanup())[0], errors[0]);
  assert.equal(h.calls.length, count); assert.equal(h.restoreCalls(), 1);
});
test('restoration and both A read failures preserve independent errors and B evidence', async () => {
  const h = spendReadModel({unknownRestore: true, finalReadFailure: true}); await failed(h, 'admin_spend_read_finality');
  const errors = await h.cleanup(); assert.equal(errors[0].errors.length, 3);
  assert.match(messages(errors[0]), /restore_unknown/); assert.match(messages(errors[0]), /final_read_failed/);
  assert.deepEqual(observations(h, 'admin_spend_final_settings').map(o => o.am), ['b']);
  assert.deepEqual(observations(h, 'admin_spend_final_spend').map(o => o.am), ['b']);
});
for (const [option, code] of [['changedRestoration', 'admin_spend_restore_settings'],
  ['regressedRevision', 'admin_spend_restore_revision'],
  ['changedRestoredLedger', 'admin_spend_restore_accounting'], ['splitRestoreRevision', 'admin_spend_restore_peer_mismatch']]) {
  test(option + ' cannot pass contributor finality', async () => {
    const h = spendReadModel({[option]: true}); await failed(h, 'admin_spend_read_finality');
    const errors = await h.cleanup(); assert.match(messages(errors[0]), new RegExp(code)); assert.equal(h.restoreCalls(), 1);
  });
}
for (const interruptedStep of ['step_1', 'step_2', 'expected_result']) {
  test('interruption before ' + interruptedStep + ' unwinds mode custody outside active steps', async () => {
    const h = spendReadModel({interruptedStep}); await failed(h, 'step_interrupted');
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.mode(), 'enabled');
    assert.equal(h.restoreCalls(), interruptedStep === 'step_1' ? 0 : 1);
  });
}
test('parent disposal still runs and its error is retained after contributor finality fails', async () => {
  const h = spendReadModel({unknownRestore: true}); let disposed = 0;
  h.f.cleanup = async () => {disposed++; throw Error('parent_disposal_failed');};
  await failed(h, 'admin_spend_read_finality'); const errors = await h.cleanup();
  assert.equal(disposed, 1); assert.equal(errors.length, 2); assert.match(messages(errors[1]), /parent_disposal_failed/);
});

async function goodRead() {
  const h = spendReadModel(); await run(h); await h.cleanup();
  return {state: structuredClone(h.f.expectedEnabled),
    settings: structuredClone(observations(h, 'admin_spend_enabled_settings')[0].response.body)};
}
test('exact accounting levels are calculated from current public policy at safe-integer boundaries', async () => {
  const {state, settings} = await goodRead(); settings.settings.model_budget.platform.period_cap = Number.MAX_SAFE_INTEGER;
  state.levels = {warning: 5404319552844594, stop: 7205759403792792, disposable: 7205759403792692};
  assert.doesNotThrow(() => verifySpendRead(state, settings));
  state.levels.stop++; assert.throws(() => verifySpendRead(state, settings), /admin_spend_levels_mismatch/);
});
test('policy validation refuses malformed integers, cloud configuration and period fields', async () => {
  const {state, settings} = await goodRead();
  for (const [field, value] of [['period_cap', 0], ['period_cap', Number.MAX_SAFE_INTEGER + 1],
    ['warning_percent', 0], ['warning_percent', 90], ['stop_percent', 101], ['protected_reserve', -1],
    ['lag_allowance', 1.5], ['period_start', '2026-02-30T00:00:00Z'], ['period_end', '2000-01-01T00:00:00Z']]) {
    const current = structuredClone(settings); current.settings.model_budget.platform[field] = value;
    assert.throws(() => verifySpendRead(state, current), /admin_spend_policy_shape/);
  }
  for (const field of ['run_rate_units_per_day', 'max_observation_age_seconds']) {
    const current = structuredClone(settings); current.settings.model_budget.platform.cloud[field] = 0;
    assert.throws(() => verifySpendRead(state, current), /admin_spend_policy_shape/);
  }
  const current = structuredClone(settings); current.settings.model_budget.platform.protected_reserve = 1000;
  assert.throws(() => verifySpendRead(state, current), /admin_spend_levels_mismatch/);
  state.ledger.period_start = '2000-01-01T00:00:00Z';
  assert.throws(() => verifySpendRead(state, settings), /admin_spend_period_mismatch/);
});
test('refusal payloads reject every private accounting field even when null or zero', () => {
  for (const field of ['levels', 'committed', 'ledger', 'settings_revision', 'metering_enabled', 'cloud', 'units']) {
    for (const value of [null, 0]) assert.throws(() => verifySpendRefusal({status: 403,
      body: {code: 'forbidden', [field]: value}}, 403, 'forbidden'), /admin_spend_refusal_exposed_accounting/);
  }
});
