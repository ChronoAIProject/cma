import {EventEmitter} from 'node:events';
// Offline clock/browser/public-state model; never provides runtime fixture evidence.
import assert from 'node:assert/strict';

export function automaticModel(options = {}) {
  const calls = [], observations = [], cleanups = [], actions = [], receipts = [];
  const epoch = Date.parse('2026-11-17T00:00:00Z'), controller = new AbortController();
  const original = {inactivity: {auto_pause_enabled: false, pause_after_seconds: 900, grace_started_at: null},
    retention: {days: 30}};
  let settings = structuredClone(original), revision = 1, draft, listener, closed = false, saves = 0;
  let now = 0, enabledAt, busyReads = 0, setupCount = 0, step = false;
  const stamp = () => new Date(epoch + now).toISOString();
  const cohort = ['idle', 'busy', 'always_on'].map(kind => ({id: 'sandbox-' + kind, kind}));
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true, cohort,
    cleanup: async () => {actions.push({op: 'fixture_cleanup'});},
    waitIdleThreshold: async () => {actions.push({op: 'fixture_oracle'}); now += 60000;
      return {elapsed_seconds: 60, grace_elapsed: true};},
    readBusyWork: async () => {
      busyReads++; actions.push({op: 'busy_read', closed});
      if (options.workReadFailure && busyReads > 1) throw Error('busy_read_failed');
      return {sandbox_id: 'sandbox-busy', agent_id: 'agent-busy', turn_sequence: 3, runtime_id: 'runtime-busy',
        sequence: busyReads > 1 && !options.stalled ? 11 : 10, restarts: options.restarted && busyReads > 1 ? 1 : 0,
        ...(options.malformedWork ? {sequence: null} : {}),
        ...(options.changedWork && busyReads > 1 ? {[options.changedWork]: 'different'} : {})};
    }};
  const changed = () => saves > 0;
  const ctx = {am: 'a', signal: controller.signal, remainingMs: () => (options.budget ?? 180000) - now,
    fixture: name => name === 'browser' ? adapter : {take: async () => {setupCount++; return f;}},
    assert: (ok, code) => {assert.ok(step, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (_label, run) => {step = true; try {return await run();} finally {step = false;}},
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args)); actions.push({op: 'request', method: args.method, path: args.path, closed});
      const reply = (body, status = 200) => {
        assert.ok((args.allowed ?? [200]).includes(status), 'status_' + status);
        return {status, body: structuredClone(body), timing: {am: args.am, request_id: 'api-' + calls.length}};
      };
      if (args.path === '/api/v1/admin/settings') {
        if (args.method === 'PUT') {
          actions.push({op: 'restore', closed});
          assert.equal(args.body.expected_revision, revision);
          if (options.foreignSettings) settings.retention.days = 7;
          else {settings = structuredClone(args.body.settings); revision++;}
          if (options.restoreFailure) throw Error('restore_unknown');
        }
        if (args.method === 'PATCH') {
          if (options.invalidAccepted) {
            settings.inactivity = structuredClone(args.body.inactivity); revision++;
            return reply({revision, settings}, 200);
          }
          return reply({code: options.wrongRefusal ? 'other_error' : 'invalid_request'}, 422);
        }
        const body = {revision, settings: structuredClone(settings)};
        if (options.stalePeer && saves && args.am !== ctx.am) body.revision--;
        if (options.peerGrace && settings.inactivity.auto_pause_enabled && args.am !== ctx.am)
          body.settings.inactivity.grace_started_at = new Date(epoch - 1000).toISOString();
        return reply(body);
      }
      const match = new URL(args.path, 'http://fixture.invalid').pathname.match(/\/sandboxes\/([^/]+)(\/events)?$/);
      assert.ok(match, args.path);
      const sandboxId = match[1], kind = cohort.find(s => s.id === sandboxId)?.kind;
      const paused = enabledAt != null && now - enabledAt >= 60000 && kind === 'idle';
      if (options.snapshotFailure && closed && kind === 'always_on') throw Error('snapshot_failed');
      if (match[2]) return reply([{sandbox_id: sandboxId, seq: 1, appended_at: new Date(epoch - 120000).toISOString(),
        kind: {type: options.protectedEvent && closed && kind !== 'idle' ? 'resumed' : 'created'}}]);
      const sandbox = {id: options.wrongId ? 'foreign' : sandboxId,
        status: paused && !options.stillActive ? 'paused' : 'active',
        lifecycle: {next_authority: paused ? 2 : 1}, work: {step: 'ready', next_fence: 1},
        profile: {always_on: kind === 'always_on'}};
      if (options.wrongPolicy && kind === 'always_on') sandbox.profile.always_on = false;
      if (options.initialOperation && !changed()) sandbox.lifecycle.operation = {purpose: 'pause'};
      if (options.pendingOperation && paused) sandbox.lifecycle.operation = {purpose: 'pause'};
      if (options.protectedEffect && closed && kind !== 'idle') sandbox.lifecycle.next_authority++;
      if (options.protectedStatus && closed && kind !== 'idle') sandbox.status = 'paused';
      return reply({sandbox});
    }};
  const adapter = {
    open: async () => {
      actions.push({op: 'open'});
      const page = Object.assign(new EventEmitter(), {
        goto: async () => {draft = structuredClone(settings); receipts.push({method: 'GET', status: 200,
          request_id: 'load-' + saves, am: ctx.am, serving_am: ctx.am});},
        getByRole: (role, spec) => ({waitFor: async () => {}, click: async () => {
          assert.equal(role, 'button'); assert.equal(spec.name, 'Save settings'); saves++;
          const body = {expected_revision: revision, settings: structuredClone(draft)};
          const enabling = !settings.inactivity.auto_pause_enabled && draft.inactivity.auto_pause_enabled;
          settings = structuredClone(draft); revision++;
          settings.inactivity.grace_started_at = settings.inactivity.auto_pause_enabled
            ? (options.badGrace ? 'invalid' : stamp()) : null;
          if (enabling) enabledAt = now;
          const response = {status: () => options.failedSave ? 503 : 200,
            url: () => 'http://127.0.0.1:19900/api/agent-manager/v1/admin/settings',
            request: () => ({method: () => 'PUT', postDataJSON: () => options.wrongBody ? {...body, expected_revision: 0} : body}),
            headerValue: async () => options.unjoinedSave ? 'unjoined' : 'save-' + saves,
            json: async () => ({revision: options.badRevision ? 0 : revision, settings: structuredClone(settings)})};
          receipts.push({method: 'PUT', status: response.status(), request_id: 'save-' + saves,
            am: ctx.am, serving_am: options.wrongServing ? (ctx.am === 'a' ? 'b' : 'a') : ctx.am,
            serving: {pod_uid: 'model'}, serving_after: {pod_uid: 'model'}, started_at: stamp(), ended_at: stamp(),
            attribution: {status: 'proven'}});
          actions.push({op: 'save', settings: structuredClone(settings)}); draft = structuredClone(settings);
          if (listener) {assert.ok(listener.predicate(response)); listener.resolve(response); listener = null;}
          if (options.unknownSave) throw Error('save_unknown');
        }}),
        getByLabel: label => ({waitFor: async () => {}, setChecked: async value => {draft.inactivity.auto_pause_enabled = value;},
          check: async () => {draft.inactivity.auto_pause_enabled = true;}, fill: async value => {
            const n = Number(value), seconds = draft.inactivity.pause_after_seconds;
            if (label === 'Pause after (seconds)') draft.inactivity.pause_after_seconds = n;
            else if (label === 'Pause after hours') draft.inactivity.pause_after_seconds = n * 3600 + seconds % 3600;
            else if (label === 'Pause after minutes') draft.inactivity.pause_after_seconds = Math.floor(seconds / 3600) * 3600 + n * 60 + seconds % 60;
            else if (label === 'Pause after seconds') draft.inactivity.pause_after_seconds = Math.floor(seconds / 60) * 60 + n;
            else throw Error('unknown_label');
          }, evaluate: async fn => fn({checkValidity: () => draft.inactivity.pause_after_seconds >= 60 && draft.inactivity.pause_after_seconds <= 86400})}),
        getByText: () => ({waitFor: async () => {}}),
        waitForResponse: predicate => new Promise(resolve => {listener = {predicate, resolve};}),
      });
      return {page, origin: 'http://127.0.0.1:19900', switchAM: async () => {}, close: async () => {
        if (closed) return; closed = true; actions.push({op: 'close'});
        if (options.lateWrite) receipts.push({method: 'PUT', status: 200, request_id: 'late', am: ctx.am, serving_am: ctx.am});
        if (options.closeFailure) throw Error('close_failed');
      }};
    },
    evidence: async () => {
      actions.push({op: 'evidence', closed});
      if (closed && options.evidenceFailure) throw Error('evidence_failed');
      return {authentication: options.bearerOnly ? 'real_am_bearer' : 'browser_session', requests: structuredClone(receipts)};
    },
  };
  const clock = {now: () => now, wall: () => epoch + now, sleep: async ms => {
    actions.push({op: 'sleep', ms}); now += ms;
    if (options.abortWait) controller.abort(Error('test_abort'));
  }};
  return {ctx, f, clock, calls, observations, actions, receipts, original, controller,
    settings: () => settings, get setupCount() {return setupCount;},
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
