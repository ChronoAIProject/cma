import { createHash } from 'node:crypto';
import { Blocked } from '../../runner.mjs';

export const digest = (value) => createHash('sha256').update(value).digest('hex');
export const button = (page, name) =>
  page.getByRole('button', { name, exact: typeof name === 'string' });
export const text = (page, value) => page.getByText(value, { exact: true });
export const agentPath = (id) => '/agents/' + encodeURIComponent(id);
export const apiAgent = (id) => '/api/v1/agents/' + encodeURIComponent(id);
export async function go(page, path) {
  await page.goto(new URL(path, page.url()).href);
}
export async function shown(ctx, locator, code) {
  await locator.waitFor({ state: 'visible', timeout: 30000 });
  ctx.assert(await locator.isVisible(), code);
}
export async function absent(ctx, locator, code) {
  await locator.waitFor({ state: 'hidden', timeout: 30000 });
  ctx.assert((await locator.count()) === 0 || !(await locator.isVisible()), code);
}
export async function account(page) {
  const menu = page.getByRole('menu', { name: 'Account', exact: true });
  if (!(await menu.isVisible())) await button(page, /^Account menu for /).click();
  return menu;
}
export async function pick(page, label, option) {
  await button(page, label).click();
  await page
    .getByRole('menu', { name: label, exact: true })
    .getByRole('menuitemradio', { name: option, exact: true })
    .click();
}
export function define(suffix, scope, handlers) {
  const steps = handlers.map((_, i) =>
    i === handlers.length - 1 ? 'expected_result' : `step_${i + 1}`,
  );
  return {
    id: `C275:cma-user.${suffix}`,
    scope,
    steps,
    fixtures: ['browser', 'user-fixtures'],
    async run(ctx) {
      const state = { scope };
      for (let i = 0; i < steps.length; i += 1)
        await ctx.step(steps[i], async () => {
          await handlers[i](ctx, state);
          if (i === steps.length - 1 && state.page) await browserEvidence(ctx, state);
        });
    },
  };
}
export async function setup(ctx, state, name) {
  const data = await ctx.fixture('user-fixtures').take(name, ctx);
  // Custody is registered before inspecting data or opening a browser.
  if (typeof data?.cleanup === 'function') ctx.defer('fixture_cleanup', () => data.cleanup());
  ctx.assert(typeof data?.cleanup === 'function', 'fixture_cleanup_required');
  state.data = data;
  state.browser = ctx.fixture('browser');
  const session = await state.browser.open({ actor: 'user', am: ctx.am, agentId: data.agentId });
  ctx.defer('browser_close', () => session.close());
  state.session = session;
  state.page = session.page;
  ctx.assert(
    typeof session.switchAM === 'function' && typeof session.close === 'function',
    'browser_contract',
  );
  ctx.assert(typeof state.browser.evidence === 'function', 'browser_evidence_contract');
  const origin = session.origin ?? new URL(state.page.url()).origin;
  state.origin = origin;
  // A just-created Playwright page may be about:blank; seed its actual UI origin.
  if (state.page.url() === 'about:blank') await state.page.goto(origin);
  if (name === 'account-authentication') {
    const evidence = await state.browser.evidence(state.page);
    if (evidence.authentication !== 'browser_session')
      throw new Blocked('browser_session_authentication_required');
  }
  return data;
}
export async function peer(ctx, state, path) {
  const before = await ctx.observe('browser_before_peer_read', () =>
    state.browser.evidence(state.page),
  );
  const relevant = before.requests.filter(
    (request) => !state.data?.agentId || request.agent_id_sha256 === digest(state.data.agentId),
  );
  const writes = relevant.filter((request) => !['GET', 'HEAD', 'OPTIONS'].includes(request.method));
  const from = (writes.at(-1) ?? relevant.at(-1))?.am ?? ctx.am;
  const next = state.browser.otherAM(from);
  ctx.assert(next !== from && ['a', 'b'].includes(next), 'distinct_peer_ingress');
  await state.session.switchAM(next);
  state.peerRequested = true;
  state.readAm = next;
  if (path) await go(state.page, path);
  else await state.page.reload();
  await ctx.observe('peer_browser_ingress', async () => ({
    from,
    to: next,
    pathname: new URL(state.page.url()).pathname,
  }));
}
export async function readAgent(ctx, state) {
  const response = await ctx.request({
    method: 'GET',
    am: state.readAm,
    path: apiAgent(state.data.agentId),
    agentId: state.data.agentId,
    allowed: [200],
  });
  ctx.assert(response.body.id === state.data.agentId, 'exact_agent_readback');
  return response.body;
}
export async function observeDOM(ctx, state, label, locator) {
  const content = await locator.innerText();
  ctx.assert(content.trim().length > 0, 'visible_content_nonempty');
  await ctx.observe(label, async () => ({
    path: new URL(state.page.url()).pathname,
    text_sha256: digest(content),
  }));
}
export async function ready(ctx, state) {
  await shown(ctx, state.page.getByLabel('Message', { exact: true }), 'message_input_visible');
  ctx.assert(
    await state.page.getByLabel('Message', { exact: true }).isEnabled(),
    'message_input_enabled',
  );
}
export async function uiWrite(ctx, state, method, path, action, allowed = [200, 202]) {
  const pending = state.page.waitForResponse(
    (response) =>
      response.request().method() === method && new URL(response.url()).pathname === path,
    { timeout: 30000 },
  );
  const [response] = await Promise.all([pending, action()]);
  const body = await response.json();
  const submitted = response.request().postDataJSON();
  ctx.assert(allowed.includes(response.status()), 'ui_write_status');
  await ctx.observe('ui_write_receipt', async () => ({
    method,
    path,
    status: response.status(),
    request_id: response.headers()['x-request-id'],
    body,
    submitted,
  }));
  return { body, submitted };
}
export async function transcript(ctx, state) {
  const response = await ctx.request({
    method: 'GET',
    am: state.readAm,
    path: apiAgent(state.data.agentId) + '/transcript',
    agentId: state.data.agentId,
  });
  ctx.assert(
    response.body.agent_id === state.data.agentId && Array.isArray(response.body.turns),
    'transcript_turns_shape',
  );
  ctx.assert(response.body.next === undefined, 'fixture_transcript_not_truncated');
  return response.body.turns;
}
export function watchWrites(ctx, state, path) {
  const requests = [];
  const listener = (request) => {
    if (request.method() === 'POST' && new URL(request.url()).pathname === path)
      requests.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() });
  };
  state.page.on('request', listener);
  ctx.defer('detach_request_witness', async () => state.page.off('request', listener));
  return requests;
}
export async function browserEvidence(ctx, state) {
  const evidence = await ctx.observe('browser_actual_receipts', () =>
    state.browser.evidence(state.page),
  );
  ctx.assert(
    evidence.schema === 'multi-am-browser-evidence-v1' &&
      evidence.real_ui === true &&
      /^[a-f0-9]{40}$/.test(evidence.compiled_source) &&
      /^sha256:[a-f0-9]{64}$/.test(evidence.compiled_manifest) &&
      evidence.secrets_redacted === true,
    'actual_compiled_browser_evidence',
  );
  if (state.scope === 'agent') {
    const requests = evidence.requests.filter(
      (request) => request.agent_id_sha256 === digest(state.data.agentId),
    );
    ctx.assert(
      requests.length > 0 &&
        requests.every((request) => request.request_id && request.owner_stable === true),
      'browser_agent_request_attribution',
    );
    if (state.peerRequested) {
      const reads = requests.filter((request) => request.method === 'GET');
      const writes = requests.filter(
        (request) => !['GET', 'HEAD', 'OPTIONS'].includes(request.method),
      );
      if (writes.length)
        ctx.assert(
          writes.some((write) => reads.some((read) => read.am !== write.am)),
          'write_one_am_read_other_am',
        );
      else
        ctx.assert(
          new Set(reads.map((request) => request.am)).size === 2,
          'actual_agent_reads_on_both_ams',
        );
    }
  }
  return evidence;
}
