import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';

// Offline model of the actual SettingsEditor state transitions, not a browser.
const platformLabels = {
  period_cap: 'Platform period cap (accounting units)', period_start: 'Period start (inclusive, RFC3339)',
  period_end: 'Period end (exclusive, RFC3339)', warning_percent: 'Warning threshold (%)',
  stop_percent: 'Hard-stop threshold (%)', protected_reserve: 'Protected reserve for existing users (accounting units)',
  lag_allowance: 'Unmetered-lag allowance (accounting units)',
};
const ownerLabels = {
  owner_day_limit: 'Per-owner daily cap (accounting units)', turn_limit: 'Per-turn cap (accounting units)',
  max_calls_per_turn: 'Maximum model calls per turn', day_timezone: 'Day-boundary timezone (IANA)',
  input_token_units: 'Input token valuation (accounting units per token)',
  output_token_units: 'Output token valuation (accounting units per token)',
  max_unobserved_units: 'Unobserved turn allowance (accounting units)', usage_timeout_seconds: 'Usage reporting timeout (seconds)',
};
const scope = 'Disabling stops new admission accounting and retains configured policies. Existing reservations and stops remain effective. '
  + 'These settings do not yet establish a strict provider-spend ceiling; platform threshold actions and model-call enforcement require their backend integrations.';

export function availabilityModel(options = {}) {
  const owner = {owner_day_limit: 1000, turn_limit: 100, max_calls_per_turn: 20, day_timezone: 'UTC',
    input_token_units: 1, output_token_units: 2, max_unobserved_units: 10, usage_timeout_seconds: 60};
  const original = {retention: {agent_log_days: 90}, model_budget: {enabled: options.enabled ?? false,
    platform: {period_cap: 10000, period_start: '2026-11-17T00:00:00Z', period_end: '2026-11-18T00:00:00Z',
      warning_percent: 60, stop_percent: 80, protected_reserve: 100, lag_allowance: 10,
      cloud: {run_rate_units_per_day: 7, max_observation_age_seconds: 60}},
    default_policy: owner, owners: [{owner: 'synthetic-owner', policy: {...owner, turn_limit: 80}}],
    request_models: [{model: 'synthetic-model', input_overhead_tokens: 5}]}};
  if (options.noDefault) delete original.model_budget.default_policy;
  if (options.noOwners) original.model_budget.owners = [];
  if (options.noModels) delete original.model_budget.request_models;
  let settings = structuredClone(original), revision = 1, available = options.available ?? true;
  let draft, baseRevision, uiAvailable, current, needsReview = false, closed = false, active = false;
  const page = new EventEmitter(), calls = [], actions = [], observations = [], receipts = [], cleanups = [], waiters = [];
  const snapshot = () => ({revision, settings: structuredClone(settings), model_budget_writes_available: available,
    model_request_budget_writes_available: options.requestAvailable ?? true, platform_spend_writes_available: true});
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    setWriterAvailability: async value => {
      actions.push({op: 'writer', value, closed}); available = value;
      if (options.modeEffect) {settings.retention.agent_log_days = 7; revision++;}
      if (!value && options.unknownDisable) throw Error('disable_unknown');
      if (value && options.unknownRecovery) throw Error('recovery_unknown');
    }, cleanup: async () => {actions.push({op: 'fixture_cleanup'});}};
  const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : {take: async () => f},
    assert: (ok, code) => {assert.ok(active, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (_label, run) => {active = true; try {return await run();} finally {active = false;}},
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    defer: (label, run) => cleanups.push({label, run}),
    request: async args => {
      calls.push(structuredClone(args)); assert.equal(args.path, '/api/v1/admin/settings');
      if (args.method === 'PUT') {
        actions.push({op: 'restore', closed}); assert.equal(args.body.expected_revision, revision);
        assert.equal(available, true); settings = structuredClone(args.body.settings); revision++;
        if (options.restoreFailure) throw Error('restore_unknown');
      }
      if (closed && options.finalReadFailures?.includes(args.am)) throw Error('final_read_' + args.am);
      const body = snapshot();
      if (options.stalePeer && revision > 1 && args.am !== ctx.am) body.revision--;
      if (options.peerGate && !available && args.am !== ctx.am) body.model_budget_writes_available = true;
      return {status: 200, body, timing: {am: args.am, request_id: 'api-' + calls.length}};
    }};
  const emit = (method, body, status, responseBody, path = '/api/agent-manager/v1/admin/settings') => {
    const request = {method: () => method, url: () => 'http://127.0.0.1:19900' + path,
      postDataJSON: () => {if (options.requestBodyFailure && method === 'PUT') throw Error('request_body_failed'); return structuredClone(body);}};
    const requestId = 'browser-' + receipts.length;
    const response = {request: () => request, url: request.url, status: () => status,
      headerValue: async () => options.unjoined ? 'unjoined' : requestId,
      json: async () => {if (options.responseBodyFailure && method === 'PUT') throw Error('response_body_failed'); return structuredClone(responseBody);}};
    page.emit('request', request);
    receipts.push({method, status, request_id: requestId, am: ctx.am,
      serving_am: options.wrongServing && method === 'PUT' ? (ctx.am === 'a' ? 'b' : 'a') : ctx.am,
      serving: {pod_uid: 'model'}, serving_after: {pod_uid: 'model'}, started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
    if (options.duplicateReceipt && method === 'PUT') receipts.push(structuredClone(receipts.at(-1)));
    page.emit('response', response);
    for (const waiter of [...waiters]) if (waiter.predicate(response)) {
      waiters.splice(waiters.indexOf(waiter), 1); waiter.resolve(response);
    }
    return response;
  };
  const reread = () => {
    current = snapshot(); uiAvailable = available;
    if (options.badRefresh && available) current.revision++;
    emit('GET', undefined, 200, current);
  };
  const canSave = () => uiAvailable && !needsReview;
  const submit = async () => {
    actions.push({op: 'submit', blocked: !canSave()});
    if (!canSave() && !options.disabledDispatch) return;
    const body = {expected_revision: baseRevision, settings: structuredClone(draft)};
    if (!available) {
      if (options.refusalEffect) {settings = structuredClone(draft); revision++;}
      emit('PUT', options.wrongBody ? {...body, expected_revision: 0} : body,
        options.wrongStatus ? 500 : 503, {code: options.wrongCode ? 'unrelated_unavailable' : 'model_budget_writes_unavailable'});
      uiAvailable = false; needsReview = true; reread();
      if (options.lostDraft) draft.model_budget.enabled = original.model_budget.enabled;
      if (options.unknownRefusal) throw Error('refusal_unknown');
    } else {
      settings = structuredClone(draft); revision++;
      if (options.erasedOther) settings.retention.agent_log_days = 1;
      emit('PUT', options.wrongBody ? {...body, expected_revision: 0} : body, options.failedSave ? 503 : 200, snapshot());
      draft = structuredClone(settings); baseRevision = revision;
      if (options.unknownSave) throw Error('save_unknown');
    }
  };
  page.goto = async () => {
    draft = structuredClone(settings); baseRevision = revision; uiAvailable = available;
    emit('GET', undefined, 200, snapshot());
  };
  const values = () => {
    const b = draft.model_budget, entries = [];
    const fields = (v, labels, prefix = '') => {if (v) for (const [k, label] of Object.entries(labels)) entries.push([prefix + label, v[k]]);};
    fields(b.platform, platformLabels); fields(b.default_policy, ownerLabels);
    b.owners.forEach((entry, i) => {entries.push([`Owner override ${i + 1}: owner ID`, entry.owner]); fields(entry.policy, ownerLabels, `Owner override ${i + 1}: `);});
    (b.request_models ?? []).forEach((entry, i) => entries.push([`Request model ${i + 1}: model name`, entry.model],
      [`Request model ${i + 1}: input overhead (tokens)`, entry.input_overhead_tokens]));
    return new Map(entries);
  };
  page.getByLabel = name => ({waitFor: async () => {},
    setChecked: async value => {assert.equal(name, 'Enable spend controls'); draft.model_budget.enabled = value;},
    isChecked: async () => draft.model_budget.enabled,
    isDisabled: async () => options.editableClosed && !uiAvailable ? false : !uiAvailable,
    isVisible: async () => name !== options.hiddenField || uiAvailable,
    inputValue: async () => {
      assert.ok(values().has(name), name);
      return String(!uiAvailable && name === options.changedField ? 'changed' : values().get(name));
    }, evaluate: async fn => fn({getAttribute: () => options.missingDescription ? null : 'scope',
      ownerDocument: {getElementById: () => ({textContent: options.wrongScope ? 'A strict provider-spend ceiling is established.' : scope})}})});
  page.getByRole = (role, {name}) => ({waitFor: async () => {},
    count: async () => name.source?.startsWith('^Owner') ? draft.model_budget.owners.length + (options.extraGroup ? 1 : 0)
      : (draft.model_budget.request_models ?? []).length,
    isVisible: async () => name === 'Configure default owner policy' ? !draft.model_budget.default_policy : true,
    isDisabled: async () => name === 'Save settings' ? !canSave() : !uiAvailable,
    isEnabled: async () => name === 'Save settings' ? canSave() : uiAvailable,
    evaluate: async fn => {let job; fn({form: {requestSubmit: () => {job = submit();}}}); await job;},
    click: async () => {
      if (name === 'Save settings') return submit();
      if (name === 'Refresh settings availability') {needsReview = true; reread(); return;}
      if (name === 'Review retained draft') {assert.ok(current); baseRevision = current.revision; needsReview = false; return;}
      throw Error('unknown_button_' + name);
    }});
  page.getByText = () => ({first() {return this;}, waitFor: async () => {}, isVisible: async () => !options.hiddenScope});
  page.waitForResponse = predicate => new Promise(resolve => waiters.push({predicate, resolve}));
  const adapter = {open: async () => ({page, origin: 'http://127.0.0.1:19900', switchAM: async () => {}, close: async () => {
    if (closed) return; closed = true; actions.push({op: 'close'});
    if (options.lateWrite) emit('PUT', {}, 422, {code: 'invalid_request'});
    if (options.unresolvedLate) page.emit('request', {method: () => 'PUT', url: () => 'http://127.0.0.1:19900/api/v1/admin/settings', postDataJSON: () => ({})});
    if (options.closeFailure) throw Error('close_failed');
  }}), evidence: async () => {
    actions.push({op: 'evidence', closed}); if (closed && options.evidenceFailure) throw Error('evidence_failed');
    return {authentication: options.bearer ? 'real_am_bearer' : 'browser_session', requests: structuredClone(receipts)};
  }};
  return {ctx, f, page, calls, actions, receipts, observations, original, emit, snapshot, settings: () => settings,
    async cleanup() {const errors = []; for (const {run} of cleanups.toReversed()) try {await run();} catch (e) {errors.push(e);} return errors;}};
}
