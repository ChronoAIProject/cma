// Offline request/custody model only; no authentic visitor or runtime is created.
import assert from 'node:assert/strict';

export function visitorModel(options = {}) {
  const calls = [], captures = [], reservations = [], observations = [], cleanups = [], responses = [];
  const browserReceipts = [], uiCalls = [];
  let browserClosed = false, responseListener, draft;
  const original = {limits: {max_sandboxes_per_user: 8, visitor_max_sandboxes: 3,
    visitor_max_launches_per_day: 5}};
  let settings = structuredClone(original), revision = 1, mutations = 0;
  const outstanding = new Set(), retired = new Set();
  const intent = (kind, index) => ({actor: 'visitor', method: 'POST', key: kind + '-' + index,
    path: kind === 'sandbox' ? '/api/v1/sandboxes' : '/api/v1/triggers/public/fixture-trigger/launches',
    body: kind === 'sandbox' ? {name: 'visitor-' + index} : {repeat: 'start_new'}});
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    emptyVisitor: true, sandboxIntents: [0, 1].map(i => intent('sandbox', i)),
    launchIntents: [0, 1, 2].map(i => intent('launch', i)), cleanup: async () => {},
    custody: async (value, dispatch) => {
      reservations.push({intent: structuredClone(value), dispatch});
      outstanding.add(value.key);
      let captured = false, cleanupJob;
      const cleanup = () => cleanupJob ??= (async () => {
        if (outstanding.has(value.key)) throw Error('visitor_unknown_intent_retained');
        const accepted = captures.find(x => x.key === value.key)?.response;
        if (accepted?.status < 300) retired.add(accepted.body.id);
        if (options.cleanupFailure) throw Error('visitor_native_finality_failed');
      })();
      return {cleanup, ...(options.missingCapture ? {} : {capture: async response => {
        assert.equal(captured, false); captured = true;
        captures.push({key: value.key, response});
        if (options.captureFailure) throw Error('visitor_capture_failed');
        outstanding.delete(value.key);
      }})};
    },
    readPublicInventory: async () => ({
      sandbox_ids: options.foreignInventory ? ['unrelated-sandbox'] : ['sandbox-result-0'],
      launch_ids: options.foreignInventory ? ['unrelated-launch-0', 'unrelated-launch-1']
        : ['launch-result-0', 'launch-result-1'],
    })};
  const ctx = {am: 'a', row: {case_id: 'C275:ADM-046'},
    fixture: name => name === 'browser' ? browserAdapter : ({take: async () => f}), assert: (ok, code) => assert.ok(ok, code),
    step: async (_label, run) => run(), defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args));
      const {method, path, body, actor, key, allowed = [200]} = args;
      const reply = (status, value) => {
        const response = {status, body: structuredClone(value), timing: {am: args.am,
          request_id: 'visitor-request-' + calls.length,
          serving: {pod_uid: 'model-' + args.am}, serving_after: {pod_uid: 'model-' + args.am},
          started_at: 'model-start', ended_at: 'model-end', attribution: {status: 'proven'}}};
        if (!allowed.includes(status)) throw Object.assign(Error('status_' + status), {response});
        return response;
      };
      if (path === '/api/v1/admin/settings') {
        if (actor === 'visitor') return reply(403, {code: 'forbidden'});
        assert.equal(actor, 'admin');
        if (method !== 'GET') {
          assert.equal(body.expected_revision, revision); revision++;
          if (method === 'PATCH') settings = {...settings, limits: structuredClone(body.limits)};
          else settings = structuredClone(body.settings);
        }
        return reply(200, {revision, settings});
      }
      assert.equal(actor, 'visitor'); assert.equal(method, 'POST'); mutations++;
      const kind = key.startsWith('sandbox') ? 'sandbox' : 'launch', index = Number(key.split('-').at(-1));
      const refused = index === (kind === 'sandbox' ? 1 : 2);
      let response;
      if (refused) {
        response = reply(429, kind === 'sandbox' ? {code: 'sandbox_limit_reached'} : {
          code: options.wrongRefusalCode ? 'other_quota' : 'resource_quota',
          resource: options.wrongRefusalResource ? 'workspaces' : 'visitor_launches', limit: 2,
          reset_at: '2030-01-01T00:00:00Z'});
      } else {
        const value = {id: kind + '-result-' + (options.duplicateLaunch && kind === 'launch' ? 0 : index)};
        if (options.missingIdentity) delete value.id;
        const status = options.successStatus ?? (kind === 'launch' ? (options.launchStatus ?? 202) : 201);
        if (options.failAt === mutations && options.unknownOutcome) throw Error('visitor_unknown_ack');
        response = reply(status, value);
        if (options.failAt === mutations && options.attributionFailure)
          throw Object.assign(Error('visitor_attribution_failed'), {response});
      }
      if (options.wrongIngress) response.timing.am = args.am === 'a' ? 'b' : 'a';
      responses.push(response);
      return response;
    }};
  const browserAdapter = {
    open: async args => {
      assert.equal(args.actor, 'admin'); uiCalls.push({operation: 'open', args});
      const response = {request: () => ({method: () => 'PUT'}),
        url: () => 'http://127.0.0.1:19905/api/agent-manager/v1/admin/settings',
        status: () => 200, headerValue: async () => options.browserUnjoined ? 'unjoined' : 'browser-save'};
      const page = {
        goto: async url => {
          assert.equal(new URL(url).pathname, '/operations'); draft = structuredClone(settings);
          uiCalls.push({operation: 'goto', url});
          browserReceipts.push({method: 'GET', request_id: 'browser-load', am: 'a', serving_am: 'a'});
        },
        getByRole: (role, spec) => {
          assert.ok((role === 'region' && spec.name === 'Platform settings')
            || (role === 'button' && spec.name === 'Save settings'));
          return {waitFor: async () => {}, click: async () => {
            uiCalls.push({operation: 'save'});
            if (!options.browserNoChange) await ctx.request({actor: 'admin', method: 'PUT',
              path: '/api/v1/admin/settings', am: 'b', body: {expected_revision: revision, settings: draft}});
            browserReceipts.push({method: 'PUT', status: 200, request_id: 'browser-save', am: 'b', serving_am: 'b',
              serving: {pod_uid: 'model-b'}, serving_after: {pod_uid: 'model-b'}, started_at: 'start', ended_at: 'end',
              attribution: {status: 'proven'}});
            responseListener?.(response);
            if (options.browserSaveUnknown) throw Error('browser_save_unknown');
          }};
        },
        getByLabel: (label, options) => {
          assert.equal(options.exact, true);
          const key = {'Visitor Sandboxes': 'visitor_max_sandboxes',
            'Visitor launches per day': 'visitor_max_launches_per_day'}[label];
          assert.ok(key || label === 'Agent log retention (days)');
          return {waitFor: async () => {}, fill: async value => {
            assert.ok(key); draft.limits[key] = Number(value); uiCalls.push({operation: 'fill', label, value});
          }};
        },
        getByText: regex => {assert.ok(regex.test('Settings saved at revision 2')); return {waitFor: async () => {}};},
        waitForResponse: predicate => new Promise(resolve => {
          responseListener = value => {assert.ok(predicate(value)); resolve(value);};
        }),
      };
      return {page, origin: 'http://127.0.0.1:19905', switchAM: async () => {}, close: async () => {
        if (browserClosed) return; browserClosed = true;
        if (options.browserLateMutation) browserReceipts.push({method: 'PUT', status: 200,
          request_id: 'browser-late', am: 'b', serving_am: 'b'});
        if (options.browserCloseFailure) throw Error('browser_close_failed');
      }};
    },
    evidence: async () => {
      uiCalls.push({operation: 'evidence', closed: browserClosed});
      if (browserClosed && options.browserEvidenceFailure) throw Error('browser_evidence_failed');
      return {authentication: options.browserBearer ? 'real_am_bearer' : 'browser_session',
        requests: structuredClone(browserReceipts)};
    },
  };
  return {ctx, f, calls, captures, reservations, observations, cleanups, responses, original, retired, outstanding, uiCalls, browserReceipts,
    get settings() {return settings;},
    async cleanup() {
      const failures = [];
      for (const {run} of cleanups.toReversed()) {
        try {await run();} catch (error) {failures.push(error);}
      }
      return failures;
    }};
}
