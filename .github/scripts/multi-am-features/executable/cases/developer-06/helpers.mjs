import { Blocked } from '../../runner.mjs';
import { projectReceipt, validateReceipt, validatePeerReads } from './receipts.mjs';
import { readFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
export const hash = (value) =>
  createHash('sha256')
    .update(typeof value === 'string' || value instanceof Uint8Array ? value : JSON.stringify(value))
    .digest('hex');
export const enc = encodeURIComponent;
export const peer = (ctx) => (ctx.am === 'a' ? 'b' : 'a');
export const key = (label) => `coverage06-${label}-${randomUUID()}`;
export const button = (page, name) => page.getByRole('button', { name, exact: typeof name === 'string' });
export async function pick(page, name, option) {
  await button(page, name).click();
  await page.getByRole('menuitemradio', { name: option, exact: true }).click();
}
export async function visible(ctx, locator, code) {
  await locator.waitFor({ state: 'visible', timeout: 30000 });
  ctx.assert(await locator.isVisible(), code);
}
export async function contains(ctx, locator, value, code) {
  await until(async () => (await locator.innerText()).includes(value));
  ctx.assert((await locator.innerText()).includes(value), code);
}
export async function absent(ctx, locator, code) {
  await until(async () => (await locator.count()) === 0);
  ctx.assert((await locator.count()) === 0, code);
}
export async function until(observe, timeout = 30000) {
  const end = Date.now() + timeout;
  do {
    if (await observe()) return;
    if (Date.now() >= end) break;
    await new Promise((resolve) => setTimeout(resolve, 2000));
  } while (Date.now() < end);
  throw Error('observation_deadline');
}
export async function api(ctx, path, options = {}) {
  const request = { actor: 'user', method: 'GET', path, ...options };
  const read = ['GET', 'HEAD'].includes(request.method);
  if (request.am === undefined && (!request.agentId || read)) {
    const previous = request.agentId ? await lastAgentWrite(ctx, request.agentId) : null;
    request.am = previous ? other(previous.am) : peer(ctx);
  }
  if (request.agentId && read && request.am !== undefined) request.peerRead = true;
  const response = await ctx.request(request);
  if (request.agentId && !read) {
    (ctx.state.apiWrites ??= new Map()).set(request.agentId, response.timing);
  }
  return response;
}
const other = (am) => {
  if (!['a', 'b'].includes(am)) throw Error('observed_ingress_required');
  return am === 'a' ? 'b' : 'a';
};
async function lastAgentWrite(ctx, agentId) {
  for (const { session } of [...(ctx.state.sessions ?? [])].reverse()) {
    const receipt = await ctx.fixture('browser').evidence(session.page);
    const found = receipt.requests?.findLast(
      (r) => r.agent_id_sha256 === hash(agentId) && !['GET', 'HEAD', 'OPTIONS'].includes(r.method),
    );
    if (found) return found;
  }
  return ctx.state.apiWrites?.get(agentId);
}
export async function switchAfterWrite(ctx, session = ctx.state.b) {
  const receipt = await ctx.fixture('browser').evidence(session.page);
  validateReceipt(receipt, ctx.fixture('user-fixtures').webSource);
  const write = receipt.requests.findLast((r) => !['GET', 'HEAD', 'OPTIONS'].includes(r.method));
  if (!write) throw Error('browser_write_receipt_required');
  const next = other(write.am);
  await session.switchAM(next);
  (session.peerReads ??= []).push({ after: receipt.requests.length, am: next, write: write.request_id });
  return next;
}
// Fixture factories establish isolated preconditions only. They never execute
// a checklist step or supply a boolean feature verdict (see fixtures.json).
export async function setup(ctx, scenario, actor = 'user', options = {}) {
  const fixture = await ctx.fixture('user-fixtures').take(scenario, ctx);
  if (typeof fixture?.cleanup !== 'function') throw Error('fixture_cleanup_missing');
  ctx.defer('dispose_owned_fixture', async () => {
    const receipt = await fixture.cleanup();
    if (receipt?.complete !== true || receipt?.owned_only !== true) throw Error('fixture_cleanup_incomplete');
    await ctx.observe('fixture_cleanup', async () => receipt);
  });
  ctx.assert(fixture.local === true && fixture.run_owned === true, 'local_owned_fixture_required');
  const browser = await open(ctx, actor, fixture.agentId, options);
  ctx.state.f = fixture;
  ctx.state.b = browser;
  return { ...browser, f: fixture };
}
export async function open(
  ctx,
  actor = 'user',
  agentId,
  { requireAgent = !!agentId, requireSession = false } = {},
) {
  const session = await ctx.fixture('browser').open({ actor, am: ctx.am, agentId });
  if (!session?.page || !session?.origin || typeof session.close !== 'function')
    throw Error('browser_session_missing');
  session.traffic = trackTraffic(session.page);
  // Defers unwind in reverse registration order. Register the final receipt
  // first so the session closes and its proxy drains before evidence() reads
  // the immutable post-close receipt. This includes late mutation-body,
  // attribution and cancellation failures surfaced while the page closes.
  ctx.defer('retain_browser_receipt', async () => {
    const receipt = await ctx.fixture('browser').evidence(session.page);
    await ctx.observe('browser_finality', async () => projectReceipt(receipt));
    validateReceipt(receipt, ctx.fixture('user-fixtures').webSource, requireAgent ? agentId : undefined);
    validatePeerReads(receipt, session.peerReads);
  });
  ctx.defer('close_browser_capture', () => session.traffic.close());
  ctx.defer('close_browser_session', () => session.close());
  if (
    requireSession &&
    (await ctx.fixture('browser').evidence(session.page)).authentication !== 'browser_session'
  )
    throw new Blocked('browser_session_authentication_required');
  (ctx.state.sessions ??= []).push({ session, agentId: requireAgent ? agentId : undefined });
  return session;
}
export const canonicalPath = (path) => path.replace(/^\/api\/agent-manager\/v1(?=\/|$)/, '/api/v1');
export function trackTraffic(page) {
  const rows = [],
    pending = new Set();
  const listener = (response) => {
    const url = new URL(response.url()),
      path = canonicalPath(url.pathname);
    if (!path.startsWith('/api/v1/') && !path.startsWith('/auth/connections/')) return;
    const task = (async () => {
      const record = {
        path,
        method: response.request().method(),
        status: response.status(),
        observed_ms: performance.now(),
        request_id: response.headers()['x-request-id'],
      };
      // Only current policy is decoded; tokens, file bytes and other bodies stay out.
      if (path.startsWith('/api/v1/resource-access/') && response.status() === 200) {
        const body = await response.json();
        record.policy_revision = body.policy_revision;
        record.capabilities = body.capabilities;
      }
      rows.push(record);
    })();
    pending.add(task);
    task.then(
      () => pending.delete(task),
      () => {
        pending.delete(task);
        rows.push({ path, failure: true });
      },
    );
  };
  page.on('response', listener);
  return {
    async flush() {
      await Promise.allSettled([...pending]);
      return [...rows];
    },
    async close() {
      page.off('response', listener);
      await Promise.allSettled([...pending]);
    },
  };
}
export async function capture(page, method, path, action) {
  let resolveResponse;
  const pending = new Promise((resolve) => {
    resolveResponse = resolve;
  });
  const listener = (response) => {
    if (response.request().method() === method && canonicalPath(new URL(response.url()).pathname) === path)
      resolveResponse(response);
  };
  page.on('response', listener);
  const timer = setTimeout(() => resolveResponse(null), 45000);
  try {
    await action();
    const response = await pending;
    if (!response) throw Error('browser_response_missing');
    return {
      status: response.status(),
      body: response.status() === 204 ? null : await response.json(),
      request: response.request().postData() ? response.request().postDataJSON() : null,
      request_id: response.headers()['x-request-id'],
    };
  } finally {
    clearTimeout(timer);
    page.off('response', listener);
  }
}
// Web authentication routes execute in the authenticated page. They are not AM routes.
export async function webRequest(ctx, path, { method = 'GET', body } = {}) {
  if (!path.startsWith('/auth/')) throw Error('web_auth_route_required');
  const receipt = await ctx.fixture('browser').evidence(ctx.state.b.page);
  if (receipt.authentication !== 'browser_session')
    throw new Blocked('browser_session_authentication_required');
  const response = await capture(ctx.state.b.page, method, path, () =>
    ctx.state.b.page.evaluate(
      async ({ path, method, body }) => {
        const value = await fetch(path, {
          method,
          credentials: 'same-origin',
          headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
          ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        });
        await value.arrayBuffer();
      },
      { path, method, body },
    ),
  );
  ctx.assert(
    typeof response.request_id === 'string' && response.request_id.trim(),
    'web_request_correlation',
  );
  await ctx.observe('web_auth_response', async () => ({
    scope: 'web',
    method,
    path_sha256: hash(path),
    status: response.status,
    request_id: response.request_id,
  }));
  ctx.assert(response.status === 200, 'web_auth_status');
  return response;
}
export async function poll(ctx, path, predicate, { agentId, timeout = 240000 } = {}) {
  const end = Date.now() + timeout;
  do {
    const r = await api(ctx, path, { agentId });
    if (predicate(r.body)) return r.body;
    if (Date.now() >= end) break;
    await new Promise((resolve) => setTimeout(resolve, 2000));
  } while (Date.now() < end);
  throw Error('durable_state_deadline');
}
// Compatibility entrypoint for callers that explicitly finish inside a case.
// The normal path uses the deferred operations in open(), which lets the
// runner retain close and evidence errors independently. Explicit callers get
// the same close -> proxy drain -> evidence ordering here.
export async function finish(ctx) {
  const failures = [];
  for (const { session, agentId } of ctx.state.sessions ?? []) {
    try {
      await session.close();
    } catch (error) {
      failures.push(error);
    }
    try {
      await session.traffic?.close();
    } catch (error) {
      failures.push(error);
    }
    try {
      const receipt = await ctx.fixture('browser').evidence(session.page);
      await ctx.observe('browser_receipt', async () => projectReceipt(receipt));
      validateReceipt(receipt, ctx.fixture('user-fixtures').webSource, agentId, (value, code) =>
        ctx.assert(value, code),
      );
      validatePeerReads(receipt, session.peerReads, (value, code) => ctx.assert(value, code));
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length) throw new AggregateError(failures, 'browser_finalization_failed');
}
export function define(suffix, handlers, { fixtures = [], scope = 'browser' } = {}) {
  const id = `C275:cma-user.${suffix}`;
  const row = catalog.rows.find((r) => r.case_id === id);
  if (!row || handlers.length !== row.required_steps.length) throw Error(`case_step_shape_${suffix}`);
  return {
    id,
    scope,
    steps: [...row.required_steps],
    fixtures: ['browser', 'user-fixtures', ...fixtures],
    async run(ctx) {
      for (let i = 0; i < handlers.length; i++) {
        await ctx.step(row.required_steps[i], async () => {
          await handlers[i](ctx);
        });
      }
    },
  };
}
