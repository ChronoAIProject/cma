import { validEgReceipt, retainBrowserEvidence } from './receipts.mjs';
import { readFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import { Blocked } from '../../../runner.mjs';
import { validServingReceipt } from '../../../serving.mjs';

export const IDS = [19, 20, 21, 23, 24, 25, 26, 28, 29, 30, 44, 45, 46, 47, 48, 49, 55, 70, 71, 72];
export const catalog = JSON.parse(
  readFileSync(new URL('../../../catalog.json', import.meta.url)),
).rows;
export const same = isDeepStrictEqual;
export const enc = encodeURIComponent;
export const other = (value) => (value === 'a' ? 'b' : 'a');
export const hash = (value) =>
  createHash('sha256')
    .update(typeof value === 'string' || Buffer.isBuffer(value) ? value : JSON.stringify(value))
    .digest('hex');
export const unique = (prefix) => `${prefix}-${randomUUID()}`;
export const requireInput = (value, code) => {
  if (!value) throw new Blocked(code);
  return value;
};
export function define(number, handlers, scope = 'browser') {
  const id = `C275:EG-${String(number).padStart(3, '0')}`;
  const row = catalog.find((row) => row.case_id === id);
  if (!IDS.includes(number) || handlers.length !== row.required_steps.length)
    throw Error('eg20_step_contract');
  return {
    id,
    scope,
    ...([19, 29, 30, 45, 46, 70].includes(number) ? { evidence_scope: 'eg_only' } : {}),
    fixtures: ['user-fixtures'],
    steps: [...row.required_steps],
    async run(ctx) {
      const state = {};
      for (let i = 0; i < handlers.length; i++)
        await ctx.step(row.required_steps[i], async () => {
          await handlers[i](ctx, state);
          if (i === handlers.length - 1 && state.fx) await state.fx.evidence();
        });
    },
  };
}
export async function begin(ctx, state, name) {
  const fx = await ctx.fixture('user-fixtures').take(`eg20-${name}`, ctx);
  requireInput(typeof fx?.cleanup === 'function', 'eg20_cleanup_required');
  ctx.defer('cleanup_eg20_case', fx.cleanup);
  state.fx = fx;
  state.page = fx.session?.page;
  return fx;
}
export async function visible(ctx, locator, code) {
  await locator.waitFor({ state: 'visible', timeout: 30000 });
  ctx.assert(await locator.isVisible(), code);
}
export async function absent(ctx, locator, code) {
  await locator.waitFor({ state: 'hidden', timeout: 30000 });
  ctx.assert((await locator.count()) === 0 || !(await locator.isVisible()), code);
}
export const button = (page, name) =>
  page.getByRole('button', { name, exact: typeof name === 'string' });
export const input = (page, name) => page.getByLabel(name, { exact: true });
export const go = (fx, path) => fx.session.page.goto(new URL(path, fx.session.origin).href);
export async function choose(page, label, name) {
  await button(page, label).click();
  await page
    .getByRole('menu', { name: label, exact: true })
    .getByRole('menuitemradio', { name, exact: true })
    .click();
}
export async function toggle(page, label, value) {
  await page.getByRole('checkbox', { name: label, exact: true }).setChecked(value);
}
export async function listField(page, label, values) {
  while (await button(page, `Remove ${label} 1`).count())
    await button(page, `Remove ${label} 1`).click();
  for (let index = 0; index < values.length; index++) {
    await button(page, `Add ${label}`).click();
    await input(page, `${label} ${index + 1}`).fill(values[index]);
  }
}
export async function poll(ctx, read, done, code, timeout = 120000) {
  const end = Date.now() + timeout;
  for (;;) {
    const value = await read();
    if (done(value)) return value;
    ctx.assert(Date.now() < end, code);
    await new Promise((resolve) => setTimeout(resolve, 2000));
  }
}
export async function uiMutation(ctx, fx, { method, path, action, allowed = [200], kind }) {
  const pending = fx.session.page.waitForResponse(
    (response) =>
      response.request().method() === method && new URL(response.url()).pathname === path,
    { timeout: 125000 },
  );
  const [response] = await Promise.all([pending, action()]);
  const request = response.request(),
    submitted = request.postDataJSON();
  const body = response.status() === 204 ? null : await response.json();
  // Browser interception retained the intent BEFORE dispatch. Capture even refusals
  // here; original accepted IDs remain in parent custody if any assertion fails.
  await fx.capture({
    kind,
    status: response.status(),
    body,
    submitted,
    path,
    method,
    key: request.headers()['idempotency-key'] ?? null,
  });
  ctx.assert(allowed.includes(response.status()), 'eg20_ui_mutation_status');
  await ctx.observe('eg20_ui_mutation', async () => ({
    method,
    path_sha256: hash(path),
    status: response.status(),
    request_id: response.headers()['x-request-id'] ?? null,
    body_sha256: hash(body),
    intent_sha256: hash(submitted ?? null),
  }));
  return { body, submitted, key: request.headers()['idempotency-key'] ?? null };
}
export async function openChannel(ctx, fx, channel = fx.channel) {
  await go(fx, '/event-gateway');
  await fx.session.page
    .getByRole('tree', { name: 'Channels', exact: true })
    .getByRole('treeitem')
    .filter({ has: fx.session.page.getByText(channel.definition.name, { exact: true }) })
    .getByRole('button')
    .click();
  await visible(
    ctx,
    fx.session.page.getByRole('heading', { name: channel.definition.name, exact: true }),
    'eg20_exact_channel_visible',
  );
}
export async function selectNode(ctx, fx, label) {
  await button(fx.session.page, label).click();
  await visible(ctx, fx.session.page.locator('.channel-inspector'), 'eg20_inspector_visible');
}
export function assertIdentity(ctx, value, basis) {
  ctx.assert(
    value.record_id === basis.record_id &&
      value.channel_id === basis.channel_id &&
      Number.isSafeInteger(value.version) &&
      value.version >= basis.version,
    'eg20_exact_channel_revision',
  );
  ctx.assert(!Object.hasOwn(value.definition ?? {}, 'mutation'), 'eg20_private_custody_not_public');
}
export function assertNoSecret(ctx, value, secrets) {
  const encoded = typeof value === 'string' ? value : JSON.stringify(value);
  ctx.assert(
    secrets
      .filter((value) => typeof value === 'string' && value.length >= 8)
      .every((secret) => !encoded.includes(secret)),
    'eg20_no_credential_disclosure',
  );
}

export function validateBrowserEvidence(value) {
  const check = (condition, code) => {
    if (!condition) throw Error(code);
  };
  check(
    ['multi-am-browser-evidence-v1', 'multi-eg-browser-evidence-v1'].includes(value?.schema) &&
      value.real_ui === true &&
      value.secrets_redacted === true &&
      /^[a-f0-9]{40}$/.test(value.compiled_source ?? ''),
    'eg20_actual_browser_artifact',
  );
  check(
    Array.isArray(value.requests) &&
      Array.isArray(value.eg_requests) &&
      value.eg_requests.length > 0 &&
      value.eg_requests.every(validEgReceipt),
    'eg20_browser_eg_requests_required',
  );
  for (const receipt of value.requests)
    check(receipt.scope === 'am' && validServingReceipt(receipt), 'eg20_browser_serving_unproved');
  check(!value.failures?.length, 'eg20_browser_final_failures');
  return value;
}
export async function closeAndCapture(ctx, session) {
  const failures = [];
  try {
    await session.close();
  } catch (error) {
    failures.push(error);
  }
  // Evidence remains available after close/join. Always attempt it even if close
  // failed so late mutation/attribution failures cannot disappear from the row.
  try {
    await retainBrowserEvidence(
      ctx,
      validateBrowserEvidence(await session.evidence()),
      'eg20_browser_final',
    );
  } catch (error) {
    failures.push(error);
  }
  if (failures.length) throw new AggregateError(failures, 'eg20_browser_finalization_failed');
}
