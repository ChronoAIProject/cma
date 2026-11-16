import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';

const ownerLabels = {
  owner_day_limit: 'Per-owner daily cap (accounting units)', turn_limit: 'Per-turn cap (accounting units)',
  max_calls_per_turn: 'Maximum model calls per turn', day_timezone: 'Day-boundary timezone (IANA)',
  input_token_units: 'Input token valuation (accounting units per token)',
  output_token_units: 'Output token valuation (accounting units per token)',
  max_unobserved_units: 'Unobserved turn allowance (accounting units)', usage_timeout_seconds: 'Usage reporting timeout (seconds)',
};
const policy = (turn = 100) => ({owner_day_limit: 1000, turn_limit: turn, max_calls_per_turn: 4,
  day_timezone: 'Asia/Singapore', input_token_units: 2, output_token_units: 3,
  max_unobserved_units: 20, usage_timeout_seconds: 60});

export function defaultOwnerPolicyModel(options = {}) {
  const fPolicy = policy(), overridePolicy = policy(60);
  const original = {retention: {agent_log_days: 90}, model_budget: {enabled: options.enabled ?? true, owners: [
    {owner: 'owner-with-override', policy: overridePolicy},
  ], request_models: [{model: 'fixture-model', input_overhead_tokens: 5}],
  platform: {period_cap: 10000, period_start: '2026-11-17T00:00:00Z', period_end: '2026-11-18T00:00:00Z',
    warning_percent: 60, stop_percent: 80, protected_reserve: 100, lag_allowance: 10,
    cloud: {run_rate_units_per_day: 100, max_observation_age_seconds: 3600}}}};
  if (options.initialDefault) original.model_budget.default_policy = fPolicy;
  let settings = structuredClone(original), revision = 4, draft, closed = false, active = false, listener;
  let reservationReads = 0;
  const reservations = [{id: 'reservation-1', owner: 'owner-with-override', units: 12, status: 'active'}];
  const calls = [], actions = [], observations = [], cleanups = [], receipts = [], page = new EventEmitter();
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true, policy: fPolicy,
    overridePolicy, ownerWithoutOverride: 'owner-without-override', ownerWithOverride: 'owner-with-override', model: 'fixture-model',
    provider: 'fixture-provider', admissionSurface: 'model_request',
    readReservations: async () => {
      reservationReads++;
      return {items: options.reservationEffect && reservationReads > 1 ? [] : structuredClone(reservations)};
    },
    admitModelRequest: async (_ctx, input) => {
      const owner = typeof input === 'string' ? (input === 'with_override' ? f.ownerWithOverride : f.ownerWithoutOverride) : input.owner;
      const current = settings.model_budget;
      const requestId = 'admission-' + owner;
      if (!current.default_policy && options.wrongRemovedAdmission)
        return {status: 201, outcome: 'accepted', request_id: requestId, surface: f.admissionSurface,
          owner, model: f.model, scope: f.scope, reservation: {id: 'reservation-wrong', owner,
          model: f.model, scope: f.scope, provider: f.provider, policy: fPolicy}, policy: fPolicy,
          provider_request: {id: 'provider-wrong', reservation_id: 'reservation-wrong', owner,
            model: f.model, scope: f.scope, provider: f.provider}, provider_requests: 1};
      const selected = current.owners.find(row => row.owner === owner)?.policy ?? current.default_policy;
      if (!selected) return {status: 402, outcome: 'refused', request_id: requestId, surface: f.admissionSurface,
        owner, model: f.model, scope: f.scope, reservation: null, provider_request: null, policy: null,
        provider_requests: 0, error: {type: 'BudgetError', code: 'budget_unavailable', reason: 'unbudgeted'}};
      const reservationId = 'reservation-' + owner;
      return {status: 201, outcome: 'accepted', request_id: requestId, surface: f.admissionSurface,
        owner, model: f.model, scope: f.scope, reservation: {id: reservationId, owner, model: f.model,
          scope: f.scope, provider: f.provider, policy: structuredClone(selected)}, policy: structuredClone(selected),
        provider_request: {id: 'provider-' + owner, reservation_id: reservationId, owner, model: f.model,
          scope: f.scope, provider: f.provider}, provider_requests: 1};
    }, cleanup: async () => {actions.push({op: 'fixture_cleanup'});}};
  const snapshot = () => ({revision, settings: structuredClone(settings), model_budget_writes_available: true,
    model_request_budget_writes_available: true, platform_spend_writes_available: true});
  const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : {take: async () => f},
    assert: (ok, code) => {assert.ok(active, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (_label, run) => {active = true; try {return await run();} finally {active = false;}},
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    defer: (label, run) => cleanups.push({label, run}),
    request: async args => {
      calls.push(structuredClone(args)); assert.equal(args.path, '/api/v1/admin/settings');
      if (args.method === 'PUT') {
        actions.push({op: 'restore', closed}); assert.equal(args.body.expected_revision, revision);
        settings = structuredClone(args.body.settings); revision++;
        if (options.restoreFailure) throw Error('restore_unknown');
      }
      return {status: 200, body: snapshot(), timing: {am: args.am, request_id: 'api-' + calls.length}};
    }};
  const emit = (method, body, status, responseBody) => {
    const request = {method: () => method, url: () => 'http://127.0.0.1:19900/api/agent-manager/v1/admin/settings',
      postDataJSON: () => {if (options.bodyFailure) throw Error('body_failed'); return structuredClone(body);}};
    const requestId = 'browser-' + receipts.length;
    const response = {request: () => request, url: request.url, status: () => status,
      headerValue: async () => options.unjoined ? 'foreign' : requestId,
      json: async () => {if (options.responseFailure) throw Error('response_failed'); return structuredClone(responseBody);}};
    page.emit('request', request);
    receipts.push({method, status, request_id: requestId, path: request.url(), request_body: structuredClone(body),
      response_body: structuredClone(responseBody), am: ctx.am,
      serving_am: options.wrongServing ? (ctx.am === 'a' ? 'b' : 'a') : ctx.am,
      serving: {pod_uid: 'model'}, serving_after: {pod_uid: 'model'}, started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
    if (options.duplicate) receipts.push(structuredClone(receipts.at(-1)));
    page.emit('response', response);
    if (listener && listener.predicate(response)) {const waiter = listener; listener = null; waiter.resolve(response);}
    return response;
  };
  page.goto = async () => {draft = structuredClone(settings); emit('GET', undefined, 200, snapshot());};
  const field = label => {
    const find = value => {
      if (value.model_budget.default_policy) for (const [key, name] of Object.entries(ownerLabels)) if (name === label) return value.model_budget.default_policy[key];
      return undefined;
    };
    return {waitFor: async () => {}, fill: async value => {
      const b = draft.model_budget; if (!b.default_policy) b.default_policy = policy();
      const key = Object.entries(ownerLabels).find(([, name]) => name === label)?.[0];
      assert.ok(key, label); b.default_policy[key] = typeof b.default_policy[key] === 'number' ? Number(value) : value;
      actions.push({op: 'fill', label, value});
    }, inputValue: async () => String(find(draft) ?? ''), isVisible: async () => true,
      getAttribute: async () => null};
  };
  const submit = async () => {
    const body = {expected_revision: revision, settings: structuredClone(draft)};
    settings = structuredClone(draft); revision++;
    emit('PUT', options.wrongBody ? {...body, expected_revision: 0} : body, options.failedSave ? 503 : 200, snapshot());
  };
  page.getByLabel = label => field(label);
  page.getByRole = (role, {name}) => ({waitFor: async () => {}, isVisible: async () => {
    if (name === 'Configure default owner policy') return draft.model_budget.default_policy === undefined;
    if (name === 'Remove default owner policy') return draft.model_budget.default_policy !== undefined;
    return true;
  }, click: async () => {
    if (name === 'Configure default owner policy') draft.model_budget.default_policy = policy();
    else if (name === 'Remove default owner policy') delete draft.model_budget.default_policy;
    else if (name === 'Save settings') await submit();
  }});
  page.getByText = () => ({waitFor: async () => {}});
  page.waitForResponse = predicate => new Promise(resolve => {listener = {predicate, resolve};});
  const adapter = {open: async () => ({page, origin: 'http://127.0.0.1:19900', switchAM: async () => {}, close: async () => {
    if (closed) return; closed = true; actions.push({op: 'close'}); if (options.closeFailure) throw Error('close_failed');
  }}), evidence: async () => {actions.push({op: 'evidence', closed}); if (options.evidenceFailure) throw Error('evidence_failed');
    return {authentication: options.bearer ? 'real_am_bearer' : 'browser_session', requests: structuredClone(receipts)};}};
  return {ctx, f, page, calls, actions, observations, receipts, original, settings: () => settings,
    async cleanup() {const errors = []; for (const {run} of cleanups.toReversed()) try {await run();} catch (e) {errors.push(e);} return errors;}};
}
