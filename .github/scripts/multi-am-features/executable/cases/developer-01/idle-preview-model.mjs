import {EventEmitter} from 'node:events';
// Offline API/browser contract model; it does not prove runtime eligibility.
import assert from 'node:assert/strict';

export function previewModel(options = {}) {
  const calls = [], observations = [], cleanups = [], browserCalls = [], receipts = [];
  let clicked = false, closed = false, selectedAM = 'a', listener;
  const cohort = ['idle', 'busy', 'always_on'].map(kind => ({id: 'sandbox-' + kind, kind}));
  const count = options.previewCount ?? 1;
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    cohort, cleanup: async () => {}};
  const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : {take: async () => f},
    assert: (ok, code) => assert.ok(ok, code), step: async (_label, run) => run(),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args));
      const parsed = new URL(args.path, 'http://fixture.invalid');
      const reply = body => ({status: 200, body, timing: {am: args.am, request_id: 'request-' + calls.length}});
      if (parsed.pathname.endsWith('/auto-pause/preview')) return reply({candidates: count});
      const match = parsed.pathname.match(/\/sandboxes\/([^/]+)(\/events)?$/);
      assert.ok(match, args.path);
      const sandboxId = match[1], changed = clicked && sandboxId === cohort[0].id;
      if (!match[2]) {
        const body = {sandbox: {id: options.wrongDetail ? 'other' : sandboxId, status: 'active',
          lifecycle: {next_authority: changed && options.lifecycleChanged ? 2 : 1,
            ...(changed && options.operationChanged ? {operation: {purpose: 'pause', phase: 'checkpoint'}} : {})},
          work: {step: changed && options.workChanged ? 'pause' : 'ready', next_fence: 1, attempts: 0},
          profile: {always_on: sandboxId === 'sandbox-always_on'}}};
        if (changed && options.lateLifecycle && closed) body.sandbox.status = 'pausing';
        if (options.missingLifecycle) delete body.sandbox.lifecycle;
        return reply(body);
      }
      let length = options.eventCount ?? 2;
      if (changed && options.tailMutation) length += 2;
      const after = Number(parsed.searchParams.get('after') ?? 0), limit = Number(parsed.searchParams.get('limit'));
      const start = options.repeatedPage ? 0 : after;
      const events = Array.from({length: Math.max(0, Math.min(limit, length - start))}, (_, index) => {
        const seq = start + index + 1;
        return {sandbox_id: options.wrongEventOwner ? 'foreign' : sandboxId,
          seq: options.gap && index === 0 ? seq + 1 : seq,
          appended_at: '2026-11-17T00:00:00Z',
          kind: seq === 1 ? {type: 'created'} : {type: 'status_changed', from: 'pausing', to: 'active'}};
      });
      if (options.nonArrayEvents) return reply({items: events});
      return reply(events);
    }};
  const adapter = {
    open: async args => {
      assert.equal(args.actor, 'admin'); selectedAM = args.am;
      const response = {request: () => ({method: () => 'GET'}),
        url: () => 'http://127.0.0.1:19910/api/agent-manager/v1/admin/auto-pause/preview',
        status: () => options.failedPreview ? 503 : 200,
        headerValue: async () => options.unjoinedResponse ? 'unjoined' : 'browser-preview',
        json: async () => ({candidates: options.wrongBodyCount ? 99 : count})};
      const page = Object.assign(new EventEmitter(), {
        goto: async url => {browserCalls.push({operation: 'goto', url});
          receipts.push({method: 'GET', request_id: 'browser-load', am: selectedAM, serving_am: selectedAM, status: 200});},
        getByRole: (role, spec) => {
          assert.ok((role === 'region' && spec.name === 'Platform settings')
            || (role === 'button' && ['Preview idle Sandboxes', 'Pause idle now'].includes(spec.name)));
          return {waitFor: async () => {}, isEnabled: async () => true,
            click: async () => {
              assert.equal(spec.name, 'Preview idle Sandboxes'); clicked = true;
              browserCalls.push({operation: 'click'});
              receipts.push({method: options.eagerMutation ? 'POST' : 'GET', request_id: 'browser-preview',
                am: selectedAM, serving_am: options.wrongServing ? (selectedAM === 'a' ? 'b' : 'a') : selectedAM,
                status: response.status(), serving: {pod_uid: 'model-' + selectedAM},
                serving_after: {pod_uid: 'model-' + selectedAM}, started_at: 'start', ended_at: 'end',
                attribution: {status: 'proven'}});
              if (listener) {assert.ok(listener.predicate(response)); listener.resolve(response);}
            }};
        },
        getByLabel: label => {assert.equal(label, 'Agent log retention (days)'); return {waitFor: async () => {}};},
        getByText: value => {assert.equal(value, count + ' Sandboxes are candidates.'); return {waitFor: async () => {}};},
        waitForResponse: predicate => new Promise(resolve => {listener = {predicate, resolve};}),
      });
      return {page, origin: 'http://127.0.0.1:19910',
        switchAM: async am => {selectedAM = am; browserCalls.push({operation: 'switch', am});},
        close: async () => {
          if (closed) return; closed = true;
          if (options.lateMutation) receipts.push({method: options.lateMutation, request_id: 'browser-late',
            am: 'b', serving_am: 'b', status: 200});
          if (options.closeFailure) throw Error('preview_close_failed');
        }};
    },
    evidence: async () => {
      browserCalls.push({operation: 'evidence', closed});
      if (closed && options.evidenceFailure) throw Error('preview_evidence_failed');
      return {authentication: options.bearerOnly ? 'real_am_bearer' : 'browser_session',
        requests: structuredClone(receipts)};
    },
  };
  return {ctx, f, cohort, calls, observations, browserCalls, receipts,
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
