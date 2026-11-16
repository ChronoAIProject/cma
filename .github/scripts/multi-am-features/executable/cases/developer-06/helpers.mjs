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
  return ctx.request({ actor: 'user', am: peer(ctx), method: 'GET', path, ...options });
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
  ctx.state.f = fixture;
  ctx.state.b = browser;
  return { ...browser, f: fixture };
}
export async function open(ctx, actor = 'user', agentId) {
  const session = await ctx.fixture('browser').open({ actor, am: ctx.am, agentId });
  if (!session?.page || !session?.origin || typeof session.close !== 'function')
    throw Error('browser_session_missing');
  ctx.defer('close_browser_session', () => session.close());
  session.traffic = trackTraffic(session.page);
  ctx.defer('close_browser_capture', () => session.traffic.close());
  ctx.defer('retain_browser_receipt', async () => {
    const receipt = await ctx.fixture('browser').evidence(session.page);
    await ctx.observe('browser_finality', async () => projectReceipt(receipt));
    validateReceipt(receipt, ctx.fixture('user-fixtures').webSource, agentId);
  });
  (ctx.state.sessions ??= []).push({ session, agentId });
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
export function projectReceipt(receipt) {
  return {
    real_ui: receipt?.real_ui,
    compiled_source: receipt?.compiled_source,
    compiled_manifest: receipt?.compiled_manifest,
    secrets_redacted: receipt?.secrets_redacted,
    requests: Array.isArray(receipt?.requests)
      ? receipt.requests.map((r) => ({
          request_id: r.request_id,
          am: r.am,
          elapsed_ms: r.elapsed_ms,
          agent_id_sha256: r.agent_id_sha256,
          owner: r.owner,
          owner_epoch: r.owner_epoch,
          owner_stable: r.owner_stable,
        }))
      : [],
  };
}
export function validateReceipt(
  receipt,
  expectedSource,
  agentId,
  check = (v, c) => {
    if (!v) throw Error(c);
  },
) {
  check(
    receipt?.real_ui === true &&
      /^[a-f0-9]{40}$/.test(expectedSource ?? '') &&
      receipt.compiled_source === expectedSource,
    'compiled_real_ui_required',
  );
  check(
    Array.isArray(receipt.requests) &&
      receipt.requests.length > 0 &&
      receipt.requests.every(
        (r) =>
          typeof r.request_id === 'string' &&
          r.request_id.trim() &&
          ['a', 'b'].includes(r.am) &&
          Number.isFinite(r.elapsed_ms) &&
          r.elapsed_ms >= 0,
      ),
    'browser_request_attribution',
  );
  const bound = receipt.requests.filter((r) => r.agent_id_sha256);
  if (agentId)
    check(
      bound.some((r) => r.agent_id_sha256 === hash(agentId)),
      'browser_bound_agent_evidence',
    );
  check(
    bound.every(
      (r) => r.owner_epoch > 0 && ['a', 'b'].includes(r.owner) && r.owner !== r.am && r.owner_stable === true,
    ),
    'browser_current_non_owner_required',
  );
  check(receipt.secrets_redacted === true, 'browser_evidence_redaction');
}
export async function finish(ctx) {
  for (const { session, agentId } of ctx.state.sessions ?? []) {
    const receipt = await ctx.fixture('browser').evidence(session.page);
    await ctx.observe('browser_receipt', async () => projectReceipt(receipt));
    validateReceipt(receipt, ctx.fixture('user-fixtures').webSource, agentId, (value, code) =>
      ctx.assert(value, code),
    );
  }
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
          if (i === handlers.length - 1) await finish(ctx);
        });
      }
    },
  };
}
