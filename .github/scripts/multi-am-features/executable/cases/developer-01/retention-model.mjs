// Offline browser/settings model; production policy and logs are never touched.
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';

export function retentionModel(options = {}) {
  const original = {retention: {agent_log_days: options.originalDays ?? 90}, limits: {max_sandboxes_per_user: 3}};
  let settings = structuredClone(original), revision = 1, draft, input = '', closed = false, saveListener, active = false;
  const calls = [], actions = [], observations = [], cleanups = [], receipts = [], page = new EventEmitter();
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    cleanup: async () => {actions.push({op: 'fixture_cleanup'});}};
  const ctx = {am: 'a', assert: (ok, code) => {assert.ok(active, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (_label, run) => {active = true; try {return await run();} finally {active = false;}},
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    defer: (label, run) => cleanups.push({label, run}), fixture: name => name === 'browser' ? adapter : {take: async () => f},
    request: async args => {
      calls.push(structuredClone(args));
      const reply = (status, body) => {
        if (!(args.allowed ?? [200]).includes(status)) throw Error('status_' + status);
        return {status, body: structuredClone(body), timing: {am: args.am, request_id: 'api-' + calls.length}};
      };
      assert.equal(args.path, '/api/v1/admin/settings');
      if (args.method === 'PUT') {
        actions.push({op: 'restore', closed}); assert.equal(args.body.expected_revision, revision);
        settings = structuredClone(args.body.settings); revision++;
        if (options.restoreFailure) throw Error('restore_unknown');
      }
      if (args.method === 'PATCH') {
        if (options.invalidAccepted) {settings.retention = structuredClone(args.body.retention); revision++; return reply(200, {revision, settings});}
        if (options.invalidEffect) {settings.limits.max_sandboxes_per_user = 7; revision++;}
        return reply(422, {code: options.wrongRefusal ? 'other' : 'invalid_request'});
      }
      const body = {revision, settings: structuredClone(settings)};
      if (options.stalePeer && revision > 1 && args.am !== ctx.am) body.revision--;
      return reply(200, body);
    }};
  const valid = () => input !== '' && Number.isInteger(Number(input)) && Number(input) >= 0 && Number(input) <= 4294967295;
  const emit = (path, method, body, status = 200, responseBody = {}) => {
    const request = {url: () => 'http://127.0.0.1:19900' + path, method: () => method,
      postDataJSON: () => structuredClone(body)};
    page.emit('request', request);
    const requestId = 'browser-' + receipts.length;
    const response = {url: request.url, request: () => request, status: () => status,
      headerValue: async () => options.unjoined ? 'foreign' : requestId,
      json: async () => {if (options.bodyFailure) throw Error('body_failed'); return structuredClone(responseBody);}};
    receipts.push({method, status, request_id: requestId, am: ctx.am, serving_am: options.wrongServing ? (ctx.am === 'a' ? 'b' : 'a') : ctx.am,
      serving: {pod_uid: 'model'}, serving_after: {pod_uid: 'model'}, started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
    page.emit('response', response); return response;
  };
  const field = {waitFor: async () => {}, fill: async value => {
    actions.push({op: 'fill', value}); input = value; draft.retention.agent_log_days = value === '' ? NaN : Number(value);
  },
    inputValue: async () => input,
    evaluate: async fn => fn({checkValidity: () => options.boundaryInvalid ? false : options.invalidFormValid ? true : valid(), value: input,
      type: 'number', min: '0', max: '4294967295', step: '1', required: true, ...options.fieldContract})};
  page.goto = async () => {
    draft = structuredClone(settings); input = String(draft.retention.agent_log_days);
    emit('/api/v1/admin/settings', 'GET', undefined, 200, {revision, settings});
    if (options.prefetch) emit('/api/v1/admin/usage/metrics/reports', 'POST', {from: 1, to: 2},
      options.prefetch.status ?? 202, options.prefetch.body ?? {report_id: 'background-report', status: 'pending'});
  };
  page.getByLabel = name => {assert.equal(name, 'Agent log retention (days)'); return field;};
  page.getByText = () => ({waitFor: async () => {}});
  page.getByRole = (role, spec) => ({waitFor: async () => {}, click: async () => {
    assert.equal(role, 'button'); assert.equal(spec.name, 'Save settings'); actions.push({op: 'click', valid: valid()});
    if (!valid() && !options.invalidDispatch) return;
    const requestBody = {expected_revision: revision, settings: structuredClone(draft)};
    if (valid()) {
      settings = structuredClone(draft); revision++;
      if (options.erasedOther) settings.limits.max_sandboxes_per_user = 99;
    }
    const response = emit('/api/agent-manager/v1/admin/settings', 'PUT',
      options.wrongBody ? {...requestBody, expected_revision: 0} : requestBody,
      valid() ? (options.failedSave ? 503 : 200) : 422, valid() ? {revision, settings} : {code: 'invalid_request'});
    if (saveListener) {assert.ok(saveListener.predicate(response)); saveListener.resolve(response); saveListener = null;}
    if (options.unknownSave) throw Error('save_unknown');
  }});
  page.waitForResponse = predicate => new Promise(resolve => {saveListener = {predicate, resolve};});
  const adapter = {open: async () => ({page, origin: 'http://127.0.0.1:19900', switchAM: async () => {}, close: async () => {
    if (closed) return; closed = true; actions.push({op: 'close'});
    if (options.lateWrite) emit('/api/v1/admin/settings', 'PUT', {}, 422, {code: 'invalid_request'});
    if (options.closeFailure) throw Error('close_failed');
  }}), evidence: async () => {
    actions.push({op: 'evidence', closed}); if (closed && options.evidenceFailure) throw Error('evidence_failed');
    return {authentication: options.bearer ? 'real_am_bearer' : 'browser_session', requests: structuredClone(receipts)};
  }};
  return {ctx, f, page, field, calls, actions, receipts, observations, original, emit, settings: () => settings,
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
