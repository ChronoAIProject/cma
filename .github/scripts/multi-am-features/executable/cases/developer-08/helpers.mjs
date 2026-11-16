import {readFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {Blocked} from '../../runner.mjs';

const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
export const hash = value => createHash('sha256').update(value).digest('hex');
export const root = '/api/v1/documents';
export const other = am => am === 'a' ? 'b' : 'a';
export const path = (id, suffix = '') => {
  if (!/^doc_[a-f0-9]{32}$/.test(id ?? '')) throw Error('document_identity_invalid');
  return `${root}/${id}${suffix}`;
};
export const check = (value, code) => { if (!value) throw Error(code); };
export function define(name, run) {
  const row = catalog.rows.find(row => row.case_id === `C275:cma-user.documents.${name}`);
  check(row, 'document_case_missing');
  return {id: row.case_id, scope: 'browser', steps: row.required_steps,
    fixtures: ['browser', 'user-fixtures'], run};
}
export async function take(ctx, name = 'documents-basic') {
  const value = await ctx.fixture('user-fixtures').take(name, ctx);
  if (value?.scope !== 'LOCAL_DISPOSABLE_TWO_AM' || value.cleanupRegistered !== true
    || typeof value.cleanup !== 'function' || !value.actors || !value.lease) {
    throw new Blocked('document_setup_custody_missing');
  }
  return value;
}
export async function open(ctx, actor = 'user', am = ctx.am) {
  const browser = ctx.fixture('browser');
  const session = await browser.open({actor, am});
  const expected = new Set([am]);
  const switchAM = session.switchAM.bind(session);
  session.switchAM = async next => { await switchAM(next); expected.add(next); };
  ctx.defer('retain_document_browser', async () => {
    const evidence = await ctx.observe('document_browser', () => browser.evidence(session.page));
    check([...expected].every(ingress => evidence.requests.some(request => request.am === ingress)),
      'document_actual_browser_ingress_missing');
  });
  // Deferred cleanup runs in reverse order. Closing first joins outstanding
  // proxy work so its final receipts and unknown outcomes reach the evidence.
  ctx.defer('close_document_browser', () => session.close());
  return session;
}
export const goto = (session, route) => session.page.goto(new URL(route, session.origin).href);
export async function visible(ctx, locator, code) {
  await locator.waitFor({state: 'visible', timeout: 30000});
  ctx.assert(await locator.isVisible(), code);
}
export async function docPage(ctx, session, document) {
  await goto(session, `/artifacts/${document.id}`);
  await visible(ctx, session.page.getByRole('textbox', {name: 'Document title', exact: true}), 'document_opened');
  ctx.assert(new URL(session.page.url()).pathname === `/artifacts/${document.id}`, 'document_route_identity');
}
export async function read(ctx, document, actor = 'user', am = other(ctx.am)) {
  return (await ctx.request({actor, am, method: 'GET', path: path(document.id)})).body;
}
export async function source(ctx, document, actor = 'user', am = other(ctx.am)) {
  return (await ctx.request({actor, am, method: 'GET', path: path(document.id, '/source')})).body;
}
export async function response(session, method, route, action) {
  const pending = session.page.waitForResponse(value => value.request().method() === method
    && new URL(value.url()).pathname === route, {timeout: 30000});
  // Attach rejection handling before the action, including a synchronous action failure.
  const [receipt] = await Promise.all([pending, action()]);
  return receipt;
}
export async function edit(session, title, text) {
  await session.page.getByRole('textbox', {name: 'Document title', exact: true}).fill(title);
  await session.page.getByRole('textbox', {name: 'Document content', exact: true}).fill(text);
}
export async function save(ctx, session, document, status = 200) {
  const receipt = await response(session, 'PATCH', path(document.id), () =>
    session.page.getByRole('button', {name: 'Save', exact: true}).click());
  ctx.assert(receipt.status() === status, 'document_save_status');
  if (status === 200) {
    await visible(ctx, session.page.locator('.document-workspace__save-status').filter({hasText: /^Saved$/}), 'document_saved');
    return receipt.json();
  }
}
export async function pick(page, label, option) {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name: option, exact: true}).click();
}
export async function comments(ctx, session) {
  await session.page.getByRole('button', {name: 'Comments', exact: true}).click();
  const panel = session.page.getByRole('complementary', {name: 'Document comments'});
  await visible(ctx, panel, 'document_comments_open');
  return panel;
}
export async function downloadText(download) {
  const stream = await download.createReadStream();
  check(stream, 'document_download_missing');
  let size = 0; const parts = [];
  for await (const chunk of stream) {
    size += chunk.length; check(size <= 1024 * 1024, 'document_download_bound'); parts.push(chunk);
  }
  return Buffer.concat(parts).toString('utf8');
}

// Suppress one real, acknowledged upstream response. No synthetic server success.
// The UI alone decides whether/how to retry; every attempted body/key is compared.
export async function loseAcknowledgment(ctx, session, method, route, onAccepted = async () => {}, onAttempt = async () => {}) {
  const attempts = [], pending = new Set(); let accepted, fault, swallowed = false;
  const pattern = '**/api/v1/documents**';
  const handler = async intercepted => {
    const request = intercepted.request();
    if (request.method() !== method || new URL(request.url()).pathname !== route) return intercepted.fallback();
    const work = (async () => {
      const key = await request.headerValue('Idempotency-Key');
      const body = request.postData();
      check(typeof key === 'string' && key.length > 0 && typeof body === 'string', 'document_intent_missing');
      attempts.push({key, body});
      await onAttempt({method, path: route, key, body: JSON.parse(body)});
      if (swallowed) return intercepted.continue();
      swallowed = true;
      const upstream = await intercepted.fetch({maxRetries: 0});
      check(upstream.ok(), 'document_fault_requires_actual_acceptance');
      accepted = await upstream.json(); await onAccepted(accepted);
      await upstream.dispose(); await intercepted.abort('failed');
    })().catch(async error => { fault = error; await intercepted.abort('failed').catch(() => {}); });
    pending.add(work);
    try { await work; } finally { pending.delete(work); }
  };
  await session.page.route(pattern, handler);
  ctx.defer('remove_document_ack_fault', async () => {
    await session.page.unroute(pattern, handler); await Promise.all([...pending]);
    if (fault) throw fault;
  });
  return {
    async verify() {
      await Promise.all([...pending]); if (fault) throw fault;
      ctx.assert(accepted && attempts.length === 2, 'document_retry_attempt_count');
      ctx.assert(attempts[0].key === attempts[1].key && attempts[0].body === attempts[1].body,
        'document_unknown_intent_changed');
      await ctx.observe('document_same_intent_retry', async () => ({attempts: attempts.length,
        key_sha256: hash(attempts[0].key), body_sha256: hash(attempts[0].body), actual_acceptance: true}));
      return accepted;
    },
  };
}
