// Offline public Launch/settings/browser model; no worker or runtime is started.
import assert from 'node:assert/strict';

export function triggerModel(options = {}) {
  const calls = [], observations = [], cleanups = [], browserCalls = [], receipts = [], captures = [], reservations = [];
  const original = {triggers: {enabled: true, orchestration_paused: true}, retention: {days: 30}};
  let settings = structuredClone(original), revision = 1, draft, listener, closed = false, saves = 0, launch, reachedReady = false;
  const outstanding = new Set();
  const intent = key => ({actor: 'user', method: 'POST', key, path: '/api/v1/triggers/public/pub_fixture/launches',
    body: {start_if_ready: true, choice: 'new'}});
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    triggerId: 'trg_fixture', publicId: 'pub_fixture', disabledIntents: [intent('disabled-a'), intent('disabled-b')],
    launchIntent: intent('start-once'), cleanup: async () => {browserCalls.push({operation: 'fixture_cleanup'});},
    custody: async (value, dispatch) => {
      reservations.push({intent: value, dispatch}); outstanding.add(value.key); let cleanupJob;
      return {...(options.missingCapture ? {} : {capture: async response => {
        captures.push({key: value.key, response});
        if (options.captureFailure) throw Error('launch_capture_failed');
        if (response.body?.id || response.body?.code === 'triggers_disabled') outstanding.delete(value.key);
      }}), cleanup: () => cleanupJob ??= (async () => {
        if (outstanding.has(value.key)) throw Error('launch_unknown_intent_retained');
        browserCalls.push({operation: 'launch_cleanup', key: value.key});
        if (options.cleanupFailure) throw Error('launch_native_finality_failed');
      })()};
    }};
  const resolution = state => ({sandbox: {state, ...(state !== 'pending' ? {id: 'sandbox-created'} : {})},
    workspace: {state, ...(state !== 'pending' ? {id: 'workspace-created'} : {})},
    agent: {state, ...(state !== 'pending' ? {id: 'agent-created'} : {})}, choice_required: false});
  const currentLaunch = am => {
    if (!launch) return null;
    const value = structuredClone(launch);
    if (settings.triggers.orchestration_paused && !reachedReady) {
      value.blocked_reason = {type: options.wrongBlock ? 'connection_required' : 'orchestration_paused'};
      value.revision = 2;
      if (options.provisionWhilePaused) value.resolution.sandbox = {state: 'created', id: 'early-sandbox'};
    } else {
      value.state = options.failedLaunch ? 'failed_terminal' : 'ready'; reachedReady = true; value.revision = 3;
      value.ready_at = '2026-11-17T00:00:02Z'; value.resolution = resolution('created');
      if (options.wrongResource && am === 'b') value.resolution.agent.id = 'different-agent';
      if (options.missingReadyTime) delete value.ready_at;
    }
    if (options.wrongLaunchOwner) value.trigger_id = 'trg_foreign';
    if (options.changedDigest) value.revision_digest = 'b'.repeat(64);
    return value;
  };
  const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : {take: async () => f},
    assert: (ok, code) => assert.ok(ok, code), step: async (_label, run) => run(),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args));
      const url = new URL(args.path, 'http://fixture.invalid'), path = url.pathname;
      const reply = (status, body) => {
        const value = {status, body: structuredClone(body), timing: {am: args.am, request_id: 'request-' + calls.length,
          serving: {pod_uid: 'model-' + args.am}, serving_after: {pod_uid: 'model-' + args.am},
          started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}}};
        if (!(args.allowed ?? [200]).includes(status)) throw Object.assign(Error('status_' + status), {response: value});
        return value;
      };
      if (path === '/api/v1/admin/settings') {
        assert.equal(args.actor, 'admin');
        if (args.method === 'PUT') {
          assert.equal(args.body.expected_revision, revision);
          browserCalls.push({operation: 'restore', closed});
          settings = structuredClone(args.body.settings); revision++;
          if (options.restoreFailure) throw Error('settings_restore_unknown');
        }
        return reply(200, {revision, settings});
      }
      assert.equal(args.actor, 'user');
      if (path === '/api/v1/triggers/trg_fixture') return reply(200, {
        id: 'trg_fixture', public_id: 'pub_fixture', availability: 'published'});
      if (path === '/api/v1/triggers/public/pub_fixture') return reply(200, {availability: 'published',
        always_review: false, first_message: '', technical: {services: []}});
      if (path === '/api/v1/launches') {
        const value = currentLaunch(args.am), rows = value ? [value] : [];
        if (options.initialLaunch && !value) rows.push({id: 'old', trigger_id: 'trg_fixture', public_id: 'pub_fixture',
          revision_digest: 'a'.repeat(64), state: 'ready', revision: 1});
        if (options.duplicateLaunch && value && !settings.triggers.orchestration_paused) rows.push({...value, id: 'duplicate'});
        return reply(200, {items: rows, ...(options.extraPage ? {next_cursor: 'repeated'} : {})});
      }
      if (path === '/api/v1/launches/launch-once') return reply(200, currentLaunch(args.am));
      assert.equal(path, '/api/v1/triggers/public/pub_fixture/launches'); assert.equal(args.method, 'POST');
      if (!settings.triggers.enabled) {
        const r = reply(options.disabledAccepted ? 202 : 503, options.disabledAccepted ? {id: 'unexpected'}
          : {code: options.wrongRefusal ? 'unrelated_failure' : 'triggers_disabled'});
        return r;
      }
      assert.equal(args.key, 'start-once');
      launch = {id: 'launch-once', trigger_id: 'trg_fixture', public_id: 'pub_fixture',
        revision_digest: 'a'.repeat(64), state: options.draftLaunch ? 'draft' : 'resolving', revision: 1, event_head: 1,
        resolution: resolution('pending'), created_at: '2026-11-17T00:00:00Z'};
      if (options.unknownAck) throw Error('launch_ack_unknown');
      const response = reply(202, launch);
      if (options.attributionFailure) throw Object.assign(Error('launch_attribution_failed'), {response});
      return response;
    }};
  const adapter = {
    open: async args => {
      assert.equal(args.actor, 'admin');
      const page = {
        goto: async () => {draft = structuredClone(settings); receipts.push({method: 'GET', status: 200,
          request_id: 'load-' + saves, am: ctx.am, serving_am: ctx.am});},
        getByRole: (role, spec) => ({waitFor: async () => {
          assert.equal(role, 'region'); assert.equal(spec.name, 'Platform settings');
        }, click: async () => {
          assert.equal(role, 'button'); assert.equal(spec.name, 'Save settings'); saves++;
          settings = structuredClone(draft); revision++;
          const response = {status: () => options.failedSave ? 503 : 200,
            url: () => 'http://127.0.0.1:19900/api/agent-manager/v1/admin/settings',
            request: () => ({method: () => 'PUT'}), headerValue: async () => options.unjoinedSave ? 'foreign' : 'save-' + saves};
          receipts.push({method: 'PUT', status: response.status(), request_id: 'save-' + saves,
            am: ctx.am, serving_am: options.wrongServing ? (ctx.am === 'a' ? 'b' : 'a') : ctx.am,
            serving: {pod_uid: 'model'}, serving_after: {pod_uid: 'model'},
            started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
          browserCalls.push({operation: 'save', settings: structuredClone(settings)});
          if (listener) {assert.ok(listener.predicate(response)); listener.resolve(response);}
        }}),
        getByLabel: label => ({waitFor: async () => {}, setChecked: async value => {
          const key = {'Triggers enabled': 'enabled', 'Orchestration paused': 'orchestration_paused'}[label];
          assert.ok(key); draft.triggers[key] = value;
        }}),
        getByText: () => ({waitFor: async () => {}}),
        waitForResponse: predicate => new Promise(resolve => {listener = {predicate, resolve};}),
      };
      return {page, origin: 'http://127.0.0.1:19900', switchAM: async () => {}, close: async () => {
        if (closed) return; closed = true; browserCalls.push({operation: 'close'});
        if (options.lateWrite) receipts.push({method: 'PUT', status: 200, request_id: 'late', am: ctx.am, serving_am: ctx.am});
        if (options.closeFailure) throw Error('trigger_close_failed');
      }};
    },
    evidence: async () => {
      browserCalls.push({operation: 'evidence', closed});
      if (closed && options.evidenceFailure) throw Error('trigger_evidence_failed');
      return {authentication: options.bearerOnly ? 'real_am_bearer' : 'browser_session', requests: structuredClone(receipts)};
    },
  };
  return {ctx, f, calls, observations, browserCalls, receipts, captures, reservations, original,
    settings: () => settings,
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
