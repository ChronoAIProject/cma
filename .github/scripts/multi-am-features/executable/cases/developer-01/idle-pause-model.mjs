// Offline contract model only; real eligibility, work and native custody remain parent-owned.
import assert from 'node:assert/strict';

export function pauseModel(options = {}) {
  const calls = [], observations = [], cleanups = [], browserCalls = [], receipts = [];
  let clicked = false, closed = false, rejected = false, selectedAM = 'a', responseListener, dialogListener;
  let accepted = false, busyReads = 0;
  const cohort = ['idle', 'busy', 'always_on'].map(kind => ({id: 'sandbox-' + kind, kind}));
  const busyIdentity = {sandbox_id: 'sandbox-busy', agent_id: 'agent-busy', turn_sequence: 3, runtime_id: 'runtime-busy'};
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    cohort, initialSequence: 10, cleanup: async () => {browserCalls.push({operation: 'fixture_cleanup'});},
    readBusyWork: async () => {
      busyReads++; browserCalls.push({operation: 'busy_read', closed});
      if (options.workReadFailure && clicked) throw Error('busy_read_failed');
      return {...busyIdentity, sequence: clicked && !options.stalled ? 11 : 10, restarts: 0,
        ...(options.malformedInitial && !clicked ? {turn_sequence: null} : {}),
        ...(options.changedWork && clicked ? {[options.changedWork]: options.changedWork === 'turn_sequence' ? 4 : 'different'} : {}),
        ...(options.restarted && clicked ? {restarts: 1} : {})};
    }};
  const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : {take: async () => f},
    assert: (ok, code) => assert.ok(ok, code), step: async (_label, run) => run(),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args));
      const parsed = new URL(args.path, 'http://fixture.invalid');
      const reply = (body, status = 200) => ({status, body, timing: {am: args.am, request_id: 'request-' + calls.length}});
      if (parsed.pathname.endsWith('/auto-pause/pause-idle-now')) {
        assert.equal(args.method, 'POST'); assert.deepEqual(args.body, {confirmed: false}); rejected = true;
        return reply({code: options.wrongRefusal ? 'other_error' : 'invalid_admin_request'}, 400);
      }
      if (parsed.pathname.endsWith('/auto-pause/preview')) return reply({candidates: 1});
      const match = parsed.pathname.match(/\/sandboxes\/([^/]+)(\/events)?$/);
      assert.ok(match, args.path);
      const sandboxId = match[1], kind = cohort.find(s => s.id === sandboxId)?.kind;
      const changed = rejected && options.unconfirmedEffect && kind === 'idle';
      const protectedChanged = clicked && options.protectedEffect && kind !== 'idle';
      const status = clicked && kind === 'idle' ? (options.stillPausing ? 'pausing' : 'paused') : 'active';
      if (!match[2]) {
        const sandbox = {id: options.wrongDetail ? 'foreign' : sandboxId, status,
          lifecycle: {next_authority: changed || protectedChanged ? 2 : 1},
          work: {step: 'ready', next_fence: 1, attempts: 0}, profile: {always_on: kind === 'always_on'}};
        if (options.initialPause && !clicked) sandbox.lifecycle.operation = {purpose: 'pause'};
        if (options.wrongAlwaysOn && kind === 'always_on') sandbox.profile.always_on = false;
        if (options.busyAlwaysOn && kind === 'busy') sandbox.profile.always_on = true;
        if (clicked && options.pendingIdleOperation && kind === 'idle') sandbox.lifecycle.operation = {purpose: 'pause'};
        if (closed && options.lateProtectedEffect && kind !== 'idle') sandbox.lifecycle.next_authority = 2;
        return reply({sandbox});
      }
      return reply([{sandbox_id: sandboxId, seq: 1, appended_at: '2026-11-17T00:00:00Z', kind: {type: 'created'}}]);
    }};
  const adapter = {
    open: async args => {
      assert.equal(args.actor, 'admin'); selectedAM = args.am;
      const response = {
        request: () => ({method: () => 'POST', postDataJSON: () => ({confirmed: !options.wrongBody})}),
        url: () => 'http://127.0.0.1:19910/api/agent-manager/v1/admin/auto-pause/pause-idle-now',
        status: () => options.failedAck ? 503 : 200,
        headerValue: async () => options.unjoinedResponse ? 'unjoined' : 'browser-pause',
        json: async () => ({scheduled: 1, already_satisfied: options.alreadyScheduled ? 1 : 0,
          skipped: options.wrongSkipped ? 0 : 2, ...(options.ack ?? {})})};
      const page = {
        goto: async () => {browserCalls.push({operation: 'goto'});
          receipts.push({method: 'GET', request_id: 'load', status: 200, am: selectedAM, serving_am: selectedAM});
          if (options.earlyWrite) receipts.push({method: 'POST', request_id: 'early', status: 200,
            am: selectedAM, serving_am: selectedAM});},
        getByRole: (role, spec) => {
          assert.ok((role === 'region' && spec.name === 'Platform settings')
            || (role === 'button' && spec.name === 'Pause idle now'));
          return {waitFor: async () => {}, click: async () => {
            browserCalls.push({operation: 'click'});
            const dialog = {type: () => options.wrongDialogType ? 'alert' : 'confirm',
              message: () => options.wrongDialog ? 'Unrelated confirmation' :
                'Pause every currently idle Sandbox now? Active work and Always on Sandboxes are rechecked.',
              accept: async () => {accepted = true; browserCalls.push({operation: 'accept'});},
              dismiss: async () => {browserCalls.push({operation: 'dismiss'});}};
            await dialogListener(dialog);
            // A rejected dialog never dispatches. Resolve waiters so the offline
            // model can expose the primary confirmation error without real timers.
            if (!accepted) {responseListener.resolve(response); return;}
            clicked = true;
            receipts.push({method: 'POST', request_id: 'browser-pause', status: response.status(), am: selectedAM,
              serving_am: options.wrongServing ? (selectedAM === 'a' ? 'b' : 'a') : selectedAM,
              serving: {pod_uid: 'model-' + selectedAM}, serving_after: {pod_uid: 'model-' + selectedAM},
              started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
            assert.ok(responseListener.predicate(response)); responseListener.resolve(response);
          }};
        },
        getByLabel: label => {assert.equal(label, 'Agent log retention (days)'); return {waitFor: async () => {}};},
        getByText: text => {assert.equal(text, '1 Sandboxes scheduled to pause.'); return {waitFor: async () => {}};},
        waitForResponse: predicate => new Promise(resolve => {responseListener = {predicate, resolve};}),
        once: (event, listener) => {assert.equal(event, 'dialog'); dialogListener = listener;},
        waitForEvent: event => {assert.equal(event, 'dialog'); return new Promise(resolve => {
          dialogListener = async dialog => {resolve(dialog); await new Promise(done => setImmediate(done));};
        });},
      };
      return {page, origin: 'http://127.0.0.1:19910', switchAM: async am => {selectedAM = am;},
        close: async () => {
          if (closed) return; closed = true; browserCalls.push({operation: 'close'});
          if (options.lateMutation) receipts.push({method: options.lateMutation, request_id: 'late',
            am: selectedAM, serving_am: selectedAM, status: 200});
          if (options.closeFailure) throw Error('pause_close_failed');
        }};
    },
    evidence: async () => {
      browserCalls.push({operation: 'evidence', closed});
      if (closed && options.evidenceFailure) throw Error('pause_evidence_failed');
      return {authentication: options.bearerOnly ? 'real_am_bearer' : 'browser_session', requests: structuredClone(receipts)};
    },
  };
  return {ctx, f, cohort, calls, observations, browserCalls, receipts, get busyReads() {return busyReads;},
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
