import assert from 'node:assert/strict';

// Offline public-contract model, never a parent factory or accounting oracle.
export function cloudObservationModel(options = {}) {
  const through = new Date(Math.floor(Date.now() / 1000) * 1000 - 60000).toISOString().replace('.000Z', 'Z');
  const start = new Date(Date.parse(through) - 3600000).toISOString().replace('.000Z', 'Z');
  const end = new Date(Date.parse(through) + 86400000).toISOString().replace('.000Z', 'Z');
  const platform = {period_start: start, period_end: end, period_cap: 10000};
  const settings = {revision: 7, settings: {model_budget: {enabled: true, platform}},
    model_budget_writes_available: true, model_request_budget_writes_available: true};
  const emptyLedger = {period_start: start, period_end: end, settings_revision: 7,
    spent: 0, reserved: 0, adjustments: 0, closures: []};
  let state = {metering_enabled: !options.disabled, settings_revision: 7,
    levels: {warning: 8000, stop: 10000, disposable: 9000},
    ...(options.emptyLedger ? {} : {ledger: structuredClone(emptyLedger)})};
  if (options.disabled) state = {metering_enabled: false, settings_revision: 7};
  if (options.previousCloud) state.ledger.cloud = {units: 1, observed_through: options.existingInstant ? through : start,
    recorded_at: options.existingInstant ? through : start, settings_revision: 7};
  let writes = options.initialWrites ?? true, stepActive = false, restored = false, didSave = false;
  const calls = [], modes = [], observations = [], cleanups = [];
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    units: options.units ?? 10, observedThrough: through, cleanup: async () => {},
    setWritesAvailable: async enabled => {
      modes.push(enabled); if (!options.ignoreMode) writes = enabled;
      if (!enabled && options.changedModeSettings) settings.revision++;
      if (!enabled && options.unknownMode) throw Error('mode_unknown');
      if (enabled) {restored = true; if (options.unknownRestore) throw Error('restore_unknown');}
    }};
  const ctx = {am: options.am ?? 'a', fixture: () => ({take: async () => f}),
    assert: (ok, code) => {assert.ok(stepActive, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (label, run) => {
      if (options.interruptedStep === label) throw Error('step_interrupted');
      stepActive = true; try {return await run();} finally {stepActive = false;}
    },
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value: structuredClone(value)}); return value;},
    request: async args => {
      calls.push(structuredClone(args)); let status = 200, body;
      const snapshot = () => ({...structuredClone(state), ...(didSave ? {committed: f.units + (options.committedDrift ? calls.length : 0)} : {})});
      if (args.method === 'GET') {
        if (restored && options.finalReadFailure && args.am === 'a') throw Error('final_read_failed');
        if (args.path === '/api/v1/admin/settings') {
          body = {...structuredClone(settings), platform_spend_writes_available: writes};
          if (options.splitSettings && args.am === 'b') body.revision++;
          if (options.splitGate && args.am === 'b' && !writes) body.platform_spend_writes_available = true;
        } else {
          assert.equal(args.path, '/api/v1/admin/spend'); body = snapshot();
          if (options.splitLedger && args.am === 'b') {body.ledger ??= structuredClone(emptyLedger); body.ledger.spent++;}
        }
      } else {
        assert.equal(args.method, 'POST'); assert.equal(args.path, '/api/v1/admin/spend/observations');
        const value = args.body, time = Date.parse(value.observed_through);
        const invalid = !Number.isSafeInteger(value.units) || value.units < 0 || value.source !== 'cloud'
          || !Number.isFinite(time) || time > Date.now() || time < Date.parse(start) || time > Date.parse(end)
          || Object.keys(value).some(k => !['source', 'units', 'observed_through'].includes(k));
        if (args.actor !== 'admin') {status = 403; body = {code: options.wrongRoleCode ? 'different' : 'forbidden'};}
        else if (invalid) {
          status = 422; body = {code: options.wrongInvalidCode ? 'different' : 'invalid_request'};
          if (options.changedInvalid) state.ledger.reserved++;
        } else if (!writes) {
          status = 503; body = {code: options.wrongGateCode ? 'different' : 'model_budget_writes_unavailable'};
          if (options.changedGate) state.ledger.spent++;
        } else if (didSave && options.rejectRepeat) {status = 422; body = {code: 'invalid_request'};}
        else {
          if (options.unknownSaveBefore) throw Error('save_unknown');
          didSave = true; state.ledger ??= structuredClone(emptyLedger);
          state.ledger.cloud = {units: value.units, observed_through: value.observed_through,
            recorded_at: options.malformedCloud ? 'yesterday' : new Date(Math.floor(Date.now() / 1000) * 1000).toISOString().replace('.000Z', 'Z'),
            settings_revision: 7};
          if (options.changedSuccessful) state.ledger.adjustments++;
          if (options.unknownSave) throw Error('save_unknown');
          body = snapshot();
        }
        if (status === 422 && options.unknownInvalid) throw Error('invalid_unknown');
        if (status === 503 && options.unknownGate) throw Error('gate_unknown');
        if (status === 403 && options.changedRole) state.ledger.adjustments++;
        if (status === 403 && options.unknownRole) throw Error('role_unknown');
        if (options.leakedRefusal && status !== 200) body.ledger = structuredClone(state.ledger);
        if (options.refusalSuccess && status === 422) status = 200;
      }
      assert.ok((args.allowed ?? [200]).includes(status), 'status_' + status);
      return {status, body, timing: {am: args.am, request_id: 'model-' + calls.length,
        serving: {pod_uid: 'model-' + args.am}, serving_after: {pod_uid: 'model-' + args.am},
        started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}}};
    }};
  return {ctx, f, calls, modes, observations, state: () => structuredClone(state), writes: () => writes,
    async cleanup() {const errors = []; for (const {run} of cleanups.toReversed()) try {await run();} catch (e) {errors.push(e);} return errors;}};
}
