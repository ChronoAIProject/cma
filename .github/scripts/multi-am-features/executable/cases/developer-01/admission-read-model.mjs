import assert from 'node:assert/strict';

export function admissionState(revision = 22) {
  const history = []; let stop = null;
  for (let i = 1; i <= revision; i++) {
    const old = stop;
    stop = i % 2 ? {since: 1000 + i, revision: i, actor: 'model-admin', reason: 'local admission test'} : null;
    history.unshift({revision: i, at: 1000 + i, actor: 'model-admin', reason: 'local admission test', old_stop: old, new_stop: stop});
  }
  return {revision, operator_stop: structuredClone(stop), last_change: structuredClone(history[0] ?? null),
    history: structuredClone(history.slice(0, 20)), writes_available: true};
}

// Offline public-response and parent mode model; it changes no real gate.
export function admissionReadModel(options = {}) {
  let state = admissionState(options.revision ?? 22), writes = options.initialWrites ?? true, active = false, restored = false;
  const original = structuredClone(state), calls = [], modes = [], observations = [], cleanups = [];
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveAdmission: true, cleanup: async () => {},
    setWriteAvailability: async enabled => {
      modes.push(enabled);
      if (!options.ignoreMode) writes = enabled;
      if (!enabled && options.changedMode) state = admissionState(state.revision + 2);
      if (!enabled && options.unknownMode) throw Error('mode_unknown');
      if (enabled) {restored = true; if (options.unknownRestore) throw Error('restore_unknown');}
    }};
  const ctx = {am: options.am ?? 'a', fixture: () => ({take: async () => f}),
    assert: (ok, code) => {assert.ok(active, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (label, run) => {
      if (label === 'expected_result' && options.interruptedStep) throw Error('step_interrupted');
      active = true; try {return await run();} finally {active = false;}
    },
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args)); let status = 200, body;
      if (args.method === 'GET') {
        assert.equal(args.path, '/api/v1/admin/admission');
        if (restored && options.restoreReadFailure && args.am === 'a') throw Error('restored_read_failed');
        body = {...structuredClone(state), writes_available: writes,
          capacity: {active_sessions: {observed: options.capacityDrift ? calls.length : 0, bound: 100, saturated: false}}};
        if (options.splitGate && !writes && args.am === 'b') body.writes_available = true;
        if (options.splitState && args.am === 'b') body = {...admissionState(state.revision + 2), writes_available: writes};
      } else {
        assert.equal(args.method, 'POST'); assert.equal(args.path, '/api/v1/admin/admission/stop');
        assert.equal(args.body.expected_revision, state.revision);
        if (options.changedRefusal) state = admissionState(state.revision + 1);
        status = options.accepted ? 200 : 503;
        body = {code: options.wrongCode ? 'admission_control_unavailable' : 'admission_control_writes_unavailable'};
        if (options.unknownPost) throw Error('post_unknown');
      }
      assert.ok((args.allowed ?? [200]).includes(status), 'status_' + status);
      return {status, body, timing: {am: args.am, request_id: 'api-' + calls.length,
        serving: {pod_uid: 'model-' + args.am}, serving_after: {pod_uid: 'model-' + args.am},
        started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}}};
    }};
  return {ctx, f, original, calls, modes, observations, state: () => structuredClone(state), writes: () => writes,
    async cleanup() {const errors = []; for (const {run} of cleanups.toReversed()) try {await run();} catch (e) {errors.push(e);} return errors;}};
}
