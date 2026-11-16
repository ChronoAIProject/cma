import {createHash, randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {Blocked} from '../../runner.mjs';

export const equal = isDeepStrictEqual;
export const sha = value => createHash('sha256').update(value).digest('hex');
export const check = (value, code) => { if (!value) throw Error(code); };
export const requireFixture = (value, code) => { if (!value) throw new Blocked(code); };
export const unique = prefix => prefix + '-' + randomUUID();
export const id = value => {
  check(typeof value === 'string' && /^[-a-zA-Z0-9_]{1,160}$/.test(value), 'configuration_resource_id');
  return encodeURIComponent(value);
};
export const other = am => am === 'a' ? 'b' : 'a';
export const settingsBody = value => Object.fromEntries(
  ['model', 'reasoning_effort', 'permission_mode'].map(key => [key, value[key]]));

export async function poll(read, accept, code, seconds = 90) {
  const end = performance.now() + seconds * 1000;
  for (;;) {
    const value = await read();
    if (accept(value)) return value;
    check(performance.now() < end, code);
    await new Promise(resolve => setTimeout(resolve, 2000));
  }
}

export const request = (ctx, resource, method, suffix = '', body, allowed = [200], key) =>
  ctx.request({actor: 'user', agentId: resource.agentId, method,
    path: `/api/v1/agents/${id(resource.agentId)}${suffix}`, body, allowed, key});
export const read = async (ctx, resource, suffix = '') => (await request(ctx, resource, 'GET', suffix)).body;

export async function take(ctx, name) {
  const provider = ctx.fixture('user-fixtures');
  requireFixture(typeof provider.take === 'function', 'configuration_setup_provider_missing');
  const value = await provider.take(name, ctx);
  requireFixture(value?.scope === 'LOCAL_DISPOSABLE_TWO_AM' && value.createdByRun === true
    && value.custodyRegistered === true && value.agentId, 'configuration_authentic_setup_required');
  return value;
}

export async function session(ctx, resource) {
  const browser = ctx.fixture('browser');
  const value = await browser.open({actor: 'user', am: ctx.am, agentId: resource.agentId});
  requireFixture(typeof value?.close === 'function', 'configuration_browser_cleanup_missing');
  ctx.defer('configuration_browser_close', () => value.close());
  requireFixture(value.page && value.origin && typeof value.switchAM === 'function', 'configuration_real_page_required');
  await value.page.goto(value.origin + '/agents/' + id(resource.agentId));
  await value.page.getByRole('textbox', {name: 'Message', exact: true}).waitFor({state: 'visible'});
  return value;
}

export async function evidence(ctx, value, label = 'configuration_browser_evidence') {
  const observation = await ctx.observe(label, () => ctx.fixture('browser').evidence(value.page));
  ctx.assert(observation.real_ui === true && observation.requests.length > 0, 'configuration_browser_evidence_missing');
  return observation;
}

export async function openPreferences(page, mode) {
  await page.getByRole('button', {name: 'More Agent actions', exact: true}).click();
  await page.getByRole('menuitem', {name: mode === 'rename' ? 'Rename' : 'Update system prompt', exact: true}).click();
  const dialog = page.getByRole('dialog', {name: mode === 'rename' ? 'Rename Agent' : 'Update system prompt', exact: true});
  await dialog.waitFor({state: 'visible'});
  const field = dialog.getByRole('textbox', {name: mode === 'rename' ? 'Agent name' : 'Agent system prompt', exact: true});
  await field.waitFor({state: 'visible'});
  return {dialog, field, save: dialog.getByRole('button', {name: 'Save', exact: true})};
}

export async function peerRead(ctx, resource, suffix = '') {
  const am = other(ctx.am);
  const response = await ctx.request({actor: 'user', am, agentId: resource.agentId,
    method: 'GET', path: `/api/v1/agents/${id(resource.agentId)}${suffix}`});
  ctx.assert(response.timing.am === am, 'configuration_peer_read_wrong_ingress');
  return response.body;
}

export async function savePreferences(page, editor, status = 200) {
  const response = page.waitForResponse(r => r.request().method() === 'PATCH'
    && new URL(r.url()).pathname.endsWith('/preferences'));
  await editor.save.click();
  const actual = await response;
  check(actual.status() === status, 'configuration_preferences_http_status');
  return {status: actual.status(), body: await actual.json(), request: actual.request().postDataJSON()};
}

export async function conflictPreferences(page, editor, concurrentWrite) {
  // Delay the actual browser PATCH before dispatch, let a real public write
  // commit, then release that same PATCH. No response is mocked or replayed.
  let enter, release, intercepted, outcome;
  const entered = new Promise(resolve => { enter = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  const pattern = '**/api/v1/agents/*/preferences';
  const handler = async route => {
    if (route.request().method() !== 'PATCH' || intercepted) return route.fallback();
    intercepted = route.request().postDataJSON(); enter(intercepted);
    const action = await gate;
    return action === 'continue' ? route.continue() : route.abort('aborted');
  };
  await page.route(pattern, handler);
  const timeout = setTimeout(() => enter(null), 30000);
  try {
    // Attach a rejection handler immediately; cleanup joins the outstanding UI
    // operation even when the concurrent public writer fails.
    outcome = savePreferences(page, editor, 409).then(value => ({value}), error => ({error}));
    const body = await entered; check(body, 'configuration_conflict_barrier_timeout');
    const winner = await concurrentWrite(body);
    release('continue');
    const result = await outcome; if (result.error) throw result.error;
    return {conflict: result.value, winner};
  } finally {
    clearTimeout(timeout); release('abort');
    if (outcome) await outcome;
    await page.unroute(pattern, handler);
  }
}

export async function sendHeld(ctx, resource, browserSession, kind) {
  const caseId = unique('config-' + kind), marker = unique('CONFIG');
  const controls = resource.controls;
  await controls.prepareTurn({caseId, kind: 'configuration', marker});
  let released = false;
  const release = async () => { if (!released) { await controls.releaseTurn({caseId}); released = true; } };
  ctx.defer('configuration_release_held_turn', release);
  const page = browserSession.page;
  await page.getByRole('textbox', {name: 'Message', exact: true}).fill('Return this exact marker without tools: ' + marker);
  await page.getByRole('button', {name: 'Send message', exact: true}).click();
  const observed = await poll(() => controls.oracle({caseId}), value => value.case_id === caseId
    && value.request_count === 1 && value.active_streams === 1 && value.completed_effect_count === 0,
  'configuration_held_turn_timeout');
  await ctx.observe('configuration_held_provider_request', async () => observed);
  await poll(() => read(ctx, resource), value => value.status === 'working', 'configuration_busy_state_timeout');
  return {caseId, marker, release,
    async finish() {
      await release();
      const done = await poll(() => controls.oracle({caseId}), value => value.request_count === 1
        && value.active_streams === 0 && value.completed_effect_count === 1, 'configuration_turn_completion_timeout');
      await poll(() => read(ctx, resource), value => value.status === 'ready', 'configuration_idle_timeout');
      const transcript = await read(ctx, resource, '/transcript');
      check(JSON.stringify(transcript).includes(marker), 'configuration_completed_turn_not_in_transcript');
      return {done, transcript};
    }};
}
