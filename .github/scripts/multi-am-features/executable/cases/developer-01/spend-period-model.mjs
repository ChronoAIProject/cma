import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';

// Offline model of the settings wire/DOM boundary; no real resources are opened.
const labels = {
  'Platform period cap (accounting units)': 'period_cap', 'Warning threshold (%)': 'warning_percent',
  'Hard-stop threshold (%)': 'stop_percent', 'Protected reserve for existing users (accounting units)': 'protected_reserve',
  'Unmetered-lag allowance (accounting units)': 'lag_allowance',
  'Period start (inclusive, RFC3339)': 'period_start', 'Period end (exclusive, RFC3339)': 'period_end',
};
const wire = value => Object.fromEntries(Object.entries(value).map(([key, v]) =>
  [key, key.startsWith('period_') && key !== 'period_cap' ? v
    : typeof v === 'string' && /^(0|[1-9]\d*)$/.test(v) && Number.isSafeInteger(Number(v)) ? Number(v) : v]));

// Independent model of clock::seconds_from_rfc3339, not the fixture validator.
// The frontend accepts fractional seconds/offsets that this backend rejects.
const backendTime = value => {
  const match = typeof value === 'string' && value.length === 20
    && /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})Z$/.exec(value);
  if (!match) return NaN;
  const [year, month, day, hour, minute, second] = match.slice(1).map(Number);
  if (year < 1970 || month < 1 || month > 12 || day < 1 || hour > 23 || minute > 59 || second > 59) return NaN;
  const time = Date.UTC(year, month - 1, day, hour, minute, second), parsed = new Date(time);
  return parsed.getUTCFullYear() === year && parsed.getUTCMonth() === month - 1 && parsed.getUTCDate() === day ? time : NaN;
};

export function periodModel(options = {}) {
  const period = {period_cap: Number.MAX_SAFE_INTEGER, warning_percent: 60, stop_percent: 80,
    protected_reserve: 100, lag_allowance: 10, period_start: '2026-11-17T00:00:00Z', period_end: '2026-11-18T00:00:00Z'};
  const original = {retention: {agent_log_days: 90}, model_budget: {enabled: false, owners: [],
    ...(options.existing ? {platform: {...period, period_cap: 5000, cloud: {run_rate_units_per_day: 7, max_observation_age_seconds: 60}}} : {})}};
  let settings = structuredClone(original), revision = 1, draft, closed = false, active = false, listener;
  const calls = [], actions = [], observations = [], receipts = [], cleanups = [], page = new EventEmitter();
  let errors = {};
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true, period,
    invalidPeriods: [{...period, period_end: period.period_start}, {...period, warning_percent: 80},
      {...period, period_cap: '9007199254740992'}], cleanup: async () => {}};
  const ctx = {am: 'a', fixture: name => name === 'browser' ? adapter : {take: async () => f},
    assert: (ok, code) => {assert.ok(active, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (_label, run) => {active = true; try {return await run();} finally {active = false;}},
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args)); assert.equal(args.path, '/api/v1/admin/settings');
      if (args.method === 'PUT') {
        actions.push({op: 'restore', closed}); assert.equal(args.body.expected_revision, revision);
        settings = structuredClone(args.body.settings); revision++;
        if (options.restoreFailure) throw Error('restore_unknown');
      }
      const body = {revision, settings: structuredClone(settings), model_budget_writes_available: true};
      if (options.stalePeer && revision > 1 && args.am !== ctx.am) body.revision--;
      if (options.writerMissing) body.model_budget_writes_available = false;
      return {status: 200, body, timing: {am: args.am, request_id: 'api-' + calls.length}};
    }};
  const validate = p => {
    const errors = {}, x = wire(p);
    for (const key of ['period_cap', 'warning_percent', 'stop_percent', 'protected_reserve', 'lag_allowance']) {
      const min = ['protected_reserve', 'lag_allowance'].includes(key) ? 0 : 1;
      const max = key.endsWith('_percent') ? 100 : Number.MAX_SAFE_INTEGER;
      if (!Number.isSafeInteger(x[key]) || x[key] < min || x[key] > max)
        errors[key] = `Enter a whole number from ${min} to ${max}, without decimals or exponents.`;
    }
    if (x.warning_percent >= x.stop_percent) errors.warning_percent = 'Warning must be below the hard-stop percentage.';
    if (!Object.keys(errors).length && BigInt(x.protected_reserve) + BigInt(x.lag_allowance) >= BigInt(x.period_cap) * BigInt(x.stop_percent) / 100n)
      errors.protected_reserve = 'Reserve plus lag allowance must be below the hard-stop threshold.';
    for (const k of ['period_start', 'period_end']) if (!Number.isFinite(Date.parse(x[k]))
      || !/(Z|[+-]\d\d:\d\d)$/.test(x[k]) || x[k].includes('02-30'))
      errors[k] = 'Enter a valid RFC3339 instant with an explicit timezone offset.';
    if (!errors.period_start && !errors.period_end && Date.parse(x.period_start) >= Date.parse(x.period_end))
      errors.period_end = 'Period end must be after period start.';
    return errors;
  };
  const save = async () => {
    errors = validate(draft.model_budget.platform); const invalid = Object.keys(errors).length > 0;
    actions.push({op: 'click', invalid});
    if (invalid && options.invalidEffect) {settings.retention.agent_log_days = 1; revision++;}
    if (invalid && !options.invalidDispatch && !options.invalidAccepted) return;
    const desired = {...draft, model_budget: {...draft.model_budget, platform: wire(draft.model_budget.platform)}};
    const body = {expected_revision: revision, settings: structuredClone(desired)};
    const {period_start, period_end} = desired.model_budget.platform;
    const start = backendTime(period_start), end = backendTime(period_end);
    const refused = invalid || !Number.isFinite(start) || !Number.isFinite(end) || start >= end;
    if (!refused || options.invalidAccepted) {settings = structuredClone(desired); revision++;}
    if (options.erasedOther) settings.retention.agent_log_days = 1;
    const requestId = 'save-' + receipts.length, status = refused ? 422 : options.failedSave ? 503 : 200;
    const request = {method: () => 'PUT', url: () => 'http://127.0.0.1:19900/api/agent-manager/v1/admin/settings',
      postDataJSON: () => options.wrongBody ? {...body, expected_revision: 0} : body};
    const response = {request: () => request, url: request.url, status: () => status,
      headerValue: async () => options.unjoined ? 'foreign' : requestId,
      json: async () => ({revision: options.badRevision ? 0 : revision, settings: structuredClone(settings), model_budget_writes_available: true})};
    receipts.push({method: 'PUT', status, request_id: requestId, am: ctx.am,
      serving_am: options.wrongServing ? (ctx.am === 'a' ? 'b' : 'a') : ctx.am,
      serving: {pod_uid: 'model'}, serving_after: {pod_uid: 'model'}, started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
    page.emit('request', request); page.emit('response', response);
    if (listener) {assert.ok(listener.predicate(response)); listener.resolve(response); listener = null;}
    if (options.unknownSave) throw Error('save_unknown');
  };
  page.goto = async () => {draft = structuredClone(settings); receipts.push({method: 'GET', status: 200,
    request_id: 'load', am: ctx.am, serving_am: ctx.am});};
  page.getByRole = (_role, {name}) => ({waitFor: async () => {},
    isVisible: async () => name === 'Configure platform period' && !draft.model_budget.platform,
    click: async () => name === 'Configure platform period' ? draft.model_budget.platform = {} : save()});
  page.getByLabel = label => {
    const key = labels[label]; assert.ok(key || label === 'Agent log retention (days)');
    return {waitFor: async () => {}, fill: async value => {
      draft.model_budget.platform[key] = options.rounded && key === 'period_cap' ? String(Number(value) - 1) : value;
    }, inputValue: async () => String(draft.model_budget.platform[key]),
    evaluate: async fn => fn({getAttribute: name => name === 'aria-invalid' ? String(!options.errorMissing && !!errors[key]) : 'error-' + key,
      ownerDocument: {getElementById: () => ({textContent: options.wrongError ? 'unrelated field error' : errors[key]})}})};
  };
  page.locator = () => ({count: async () => Object.keys(errors).length});
  page.getByText = () => ({waitFor: async () => {}});
  page.waitForResponse = predicate => new Promise(resolve => {listener = {predicate, resolve};});
  const adapter = {open: async () => ({page, origin: 'http://127.0.0.1:19900', switchAM: async () => {}, close: async () => {
    if (closed) return; closed = true; actions.push({op: 'close'});
    if (options.lateWrite) receipts.push({method: 'PUT', status: 200, request_id: 'late', am: ctx.am, serving_am: ctx.am});
    if (options.closeFailure) throw Error('close_failed');
  }}), evidence: async () => {
    actions.push({op: 'evidence', closed}); if (closed && options.evidenceFailure) throw Error('evidence_failed');
    return {authentication: options.bearer ? 'real_am_bearer' : 'browser_session', requests: structuredClone(receipts)};
  }};
  return {ctx, f, page, calls, actions, observations, receipts, original, settings: () => settings,
    async cleanup() {const errors = []; for (const {run} of cleanups.toReversed()) try {await run();} catch (e) {errors.push(e);} return errors;}};
}
