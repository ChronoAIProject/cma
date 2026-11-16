import assert from 'node:assert/strict';

// Contract-only local model; these values never register an executable factory.
export function spendReadModel(options = {}) {
  const now = Math.floor(Date.now() / 1000) * 1000;
  const stamp = time => new Date(time).toISOString().replace('.000Z', 'Z');
  const platform = {period_start: stamp(now - 3600000), period_end: stamp(now + 86400000),
    period_cap: 1000, warning_percent: 60, stop_percent: 80, protected_reserve: 80, lag_allowance: 20,
    cloud: {run_rate_units_per_day: 1, max_observation_age_seconds: 86400}};
  const ledger = {period_start: platform.period_start, period_end: platform.period_end, settings_revision: 11,
    spent: 2, reserved: 3, adjustments: 4, closures: [],
    cloud: {units: 5, observed_through: stamp(now - 60000), recorded_at: stamp(now - 60000), settings_revision: 11}};
  const enabled = {metering_enabled: true, settings_revision: 11, levels: {warning: 600, stop: 800, disposable: 700},
    committed: 15, ledger};
  const disabled = {metering_enabled: false, settings_revision: 12};
  const missing = {...structuredClone(enabled), settings_revision: 13};
  delete missing.committed; delete missing.ledger.cloud; missing.ledger.settings_revision = 13;
  const cohorts = {enabled, disabled, enabled_without_cloud: missing};
  let mode = options.initialMode ?? 'enabled', restored = false, stepActive = false, restoreCalls = 0;
  const initialMode = mode, calls = [], modes = [], observations = [], cleanups = [];
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    expectedEnabled: structuredClone(enabled), expectedDisabled: structuredClone(disabled),
    expectedWithoutCloud: structuredClone(missing), cleanup: async () => {},
    selectMode: async next => {
      modes.push(next); if (options.ignoreMode !== next) mode = next;
      if (options.unknownMode === next) throw Error('mode_unknown');
    },
    restoreMode: async () => {
      restoreCalls++; mode = initialMode; restored = true;
      if (options.unknownRestore) throw Error('restore_unknown');
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
      assert.equal(args.method, 'GET'); calls.push({...structuredClone(args), mode});
      if (restored && options.finalReadFailure && args.am === 'a') throw Error('final_read_failed');
      let status = 200, body;
      if (args.path === '/api/v1/admin/settings') {
        const policy = {...structuredClone(platform)};
        if (mode === 'disabled' && options.withoutPolicy) delete policy.cloud;
        body = {revision: cohorts[mode]?.settings_revision ?? 14,
          settings: {model_budget: {enabled: mode !== 'disabled' || !!options.withoutPolicy, platform: policy}},
          model_budget_writes_available: true, model_request_budget_writes_available: true, platform_spend_writes_available: true};
        if (restored && options.restoredRevision) body.revision = 20;
        if (restored && options.regressedRevision) body.revision = 0;
        if (restored && options.splitRestoreRevision && args.am === 'b') body.revision = 21;
        if (restored && options.changedRestoration) body.settings.model_budget.platform.period_cap++;
        if (options.badLevelsPolicy) body.settings.model_budget.platform.warning_percent = 61;
      } else {
        assert.equal(args.path, '/api/v1/admin/spend');
        if (args.actor !== 'admin') {
          status = options.allowUser ? 200 : 403; body = {code: options.wrongRoleCode ? 'wrong' : 'forbidden'};
          if (options.leakedUser) body.ledger = structuredClone(ledger);
        } else if (mode === 'dependency_unavailable') {
          status = options.availableDependency ? 200 : 503;
          body = {code: options.wrongUnavailableCode ? 'different' : 'platform_spend_unavailable'};
          if (options.leakedUnavailable) body.ledger = structuredClone(ledger);
        } else {
          body = structuredClone(cohorts[mode]);
          if (restored && options.restoredRevision) body.settings_revision = 20;
          if (restored && options.regressedRevision) body.settings_revision = 0;
          if (restored && options.splitRestoreRevision && args.am === 'b') body.settings_revision = 21;
          if (restored && options.changedRestoredLedger && body.ledger) body.ledger.spent++;
          if (options.fabricatedZero && mode === 'enabled_without_cloud') body.committed = 0;
          if (options.nullCloud && mode === 'enabled_without_cloud') body.ledger.cloud = null;
          if (options.disabledValues && mode === 'disabled') body.committed = 0;
          if (options.changedLedger && mode === 'enabled' && modes.length) body.ledger.reserved++;
          if (options.changedCommitted && mode === 'enabled' && modes.length) body.committed++;
          if (options.splitState && args.am === 'b') body.settings_revision++;
        }
      }
      assert.ok((args.allowed ?? [200]).includes(status), 'status_' + status);
      return {status, body, timing: {am: args.am, request_id: 'model-' + calls.length,
        serving: {pod_uid: 'model-' + args.am}, serving_after: {pod_uid: 'model-' + args.am},
        started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}}};
    }};
  return {ctx, f, calls, modes, observations, mode: () => mode, restoreCalls: () => restoreCalls,
    async cleanup() {const errors = []; for (const {run} of cleanups.toReversed()) try {await run();} catch (e) {errors.push(e);} return errors;}};
}
