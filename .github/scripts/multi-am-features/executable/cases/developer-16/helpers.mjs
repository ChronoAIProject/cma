import {readFileSync} from 'node:fs';
import {createHash, randomUUID} from 'node:crypto';
const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
export const hash = value => createHash('sha256').update(typeof value === 'string' ? value : JSON.stringify(value)).digest('hex');
export const enc = encodeURIComponent;
export const peer = ctx => ctx.am === 'a' ? 'b' : 'a';
export const key = label => `coverage16-${label}-${randomUUID()}`;
export const button = (page, name) => page.getByRole('button', {name, exact: typeof name === 'string'});
export async function pick(page, name, option) {
  await button(page, name).click();
  await page.getByRole('menuitemradio', {name: option, exact: true}).click();
}
export async function visible(ctx, locator, code) {
  await locator.waitFor({state: 'visible', timeout: 30000});
  ctx.assert(await locator.isVisible(), code);
}
export async function contains(ctx, locator, value, code) {
  await waitFor(() => locator.innerText().then(text => text.includes(value)), 'ui_text_timeout');
  ctx.assert((await locator.innerText()).includes(value), code);
}
export async function absent(ctx, locator, code) {
  await waitFor(() => locator.count().then(count => count === 0), 'ui_absence_timeout');
  ctx.assert(await locator.count() === 0, code);
}
export async function api(ctx, path, options = {}) {
  return ctx.request({actor: 'user', am: peer(ctx), method: 'GET', path, ...options});
}
// Fixture factories establish isolated preconditions only. They never execute
// a checklist step or supply a boolean feature verdict (see fixtures.json).
export async function setup(ctx, scenario, actor = 'user') {
  const fixture = await ctx.fixture('user-fixtures').take(scenario, ctx);
  if (typeof fixture?.cleanup !== 'function') throw Error('fixture_cleanup_missing');
  ctx.defer('dispose_owned_fixture', async () => {
    const receipt = await fixture.cleanup();
    if (receipt?.complete !== true || receipt?.owned_only !== true) throw Error('fixture_cleanup_incomplete');
    await ctx.observe('fixture_cleanup', async () => receipt);
  });
  ctx.assert(fixture.local === true && fixture.run_owned === true, 'local_owned_fixture_required');
  const browser = await open(ctx, actor, fixture.agentId);
  ctx.state.f = fixture; ctx.state.b = browser;
  return {...browser, f: fixture};
}
export async function open(ctx, actor = 'user', agentId) {
  const session = await ctx.fixture('browser').open({actor, am: ctx.am, agentId});
  if (!session?.page || !session?.origin || typeof session.close !== 'function') throw Error('browser_session_missing');
  ctx.defer('close_browser_session', async () => { await session.trace?.finish(); await session.close(); });
  session.trace = traceResponses(session.page, session.origin);
  (ctx.state.sessions ??= []).push(session);
  return session;
}
export async function capture(page, method, path, action) {
  // Install the response listener before the one UI intent; never retry a write.
  const pending = page.waitForResponse(r => r.request().method() === method && new URL(r.url()).pathname === path, {timeout: 45000});
  const settled = pending.then(response => ({response}), error => ({error}));
  await action();
  const got = await settled;
  if (got.error) throw Error('browser_response_missing');
  const response = got.response;
  return {status: response.status(), body: await response.json(),
    request: response.request().postDataJSON(), request_id: response.headers()['x-request-id']};
}
export async function poll(ctx, path, predicate, {agentId, timeout = 240000} = {}) {
  const end = Date.now() + timeout;
  do {
    const r = await api(ctx, path, {agentId});
    if (predicate(r.body)) return r.body;
    if (Date.now() >= end) break;
    await new Promise(resolve => setTimeout(resolve, 2000));
  } while (Date.now() < end);
  throw Error('durable_state_deadline');
}
export async function finish(ctx) {
  for (const session of ctx.state.sessions ?? []) {
    const receipt = await ctx.observe('browser_receipt', () => ctx.fixture('browser').evidence(session.page));
    ctx.assert(receipt?.schema === 'multi-am-browser-evidence-v1' && receipt?.real_ui === true && receipt?.compiled_source === ctx.state.f.webSource,
      'compiled_real_ui_required');
    ctx.assert(Array.isArray(receipt.requests) && receipt.requests.length > 0 && receipt.requests.every(r =>
      r.request_id && ['a','b'].includes(r.am) && Number.isFinite(r.elapsed_ms)), 'browser_request_attribution');
    const bound = receipt.requests.filter(r => r.agent_id_sha256);
    ctx.assert(bound.every(r => r.owner_epoch > 0 && r.owner !== r.am && r.owner_stable === true),
      'browser_current_non_owner_required');
    ctx.assert(receipt.secrets_redacted === true, 'browser_evidence_redaction');

  }
}
export function define(suffix, handlers, {fixtures = [], scope = 'browser'} = {}) {
  const id = `C275:cma-user.${suffix}`;
  const row = catalog.rows.find(r => r.case_id === id);
  if (!row || handlers.length !== row.required_steps.length) throw Error(`case_step_shape_${suffix}`);
  return {id, scope, steps: [...row.required_steps], fixtures: ['browser', 'user-fixtures', ...fixtures],
    async run(ctx) {
      for (let i = 0; i < handlers.length; i++) {
        await ctx.step(row.required_steps[i], async () => {
          await handlers[i](ctx);
          if (i === handlers.length - 1) await finish(ctx);
        });
      }
    }};
}

// UI conditions poll only the local browser; public API polling remains >=2s.
export async function waitFor(predicate, code, timeout = 30000) {
  const end = Date.now() + timeout;
  do { if (await predicate()) return; await new Promise(resolve => setTimeout(resolve, 200)); }
  while (Date.now() < end);
  throw Error(code);
}
export async function attribute(ctx, locator, name, value, code) {
  await waitFor(() => locator.getAttribute(name).then(actual => actual === value), code, 240000);
  ctx.assert(await locator.getAttribute(name) === value, code);
}
// Capture actual UI usage responses without extending the shared browser API.
// Credentials and arbitrary response bodies are not captured by this recorder.
export function traceResponses(page, origin) {
  const rows = [], pending = new Set(); let sequence = 0;
  const onResponse = response => {
    const url = new URL(response.url());
    if (url.origin !== origin || !url.pathname.startsWith('/api/v1/usage/')) return;
    const seq = ++sequence, started = performance.now();
    const task = (async () => {
      const request = response.request();
      await response.finished();
      const contentType = response.headers()['content-type'] ?? '';
      if (!contentType.includes('json')) return;
      const body = await response.json();
      const timing = request.timing();
      rows.push({sequence:seq, path:url.pathname, query:url.search, method:request.method(),
        status:response.status(), body, request_id:response.headers()['x-request-id'],
        elapsed_ms:timing.responseEnd >= 0 ? timing.responseEnd : performance.now()-started});
    })().catch(() => rows.push({sequence:seq,path:url.pathname,status:response.status(),capture_error:true}));
    pending.add(task); void task.finally(() => pending.delete(task));
  };
  page.on('response', onResponse);
  return {mark:()=>sequence, async read({since=0,pathPrefix=''}={}) {
    await Promise.all([...pending]);
    return rows.filter(row => row.sequence > since && row.path.startsWith(pathPrefix)).sort((a,b)=>a.sequence-b.sequence);
  }, async finish(){page.off('response',onResponse);await Promise.all([...pending]);}};
}
