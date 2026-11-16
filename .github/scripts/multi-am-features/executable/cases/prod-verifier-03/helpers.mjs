import {readFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {Blocked} from '../../runner.mjs';

export const rows = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)))
  .rows.filter(row => row.family === 'eg');
export const digest = value => createHash('sha256').update(typeof value === 'string'
  ? value : JSON.stringify(value)).digest('hex');
export const peer = am => am === 'a' ? 'b' : 'a';
export const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
export const segment = value => encodeURIComponent(value);

export async function pages(ctx, lease, path, identity, options = {}) {
  const items = [], seen = new Set(), cursors = new Set();
  let cursor;
  for (let n = 0; n < 20; n++) {
    const page = await egRequest(ctx, lease, {...options,
      path: path + (cursor == null ? '' : `${path.includes('?') ? '&' : '?'}after=${segment(cursor)}`)});
    ctx.assert(Array.isArray(page.items), 'page_items');
    for (const item of page.items) {
      ctx.assert(typeof item[identity] === 'string' && !seen.has(item[identity]), 'page_duplicate_identity');
      seen.add(item[identity]); items.push(item);
    }
    cursor = page.next_cursor;
    if (cursor == null) return items;
    ctx.assert(!cursors.has(cursor), 'page_cursor_progress'); cursors.add(cursor);
  }
  ctx.assert(false, 'page_budget_exceeded');
}

export function define(number, scope, fixtures, run) {
  const id = `C275:EG-${String(number).padStart(3, '0')}`;
  const row = rows.find(row => row.case_id === id);
  if (!row) throw Error('eg_catalog_row_missing');
  return {id, scope, fixtures, steps: [...row.required_steps], run};
}

// Missing adapter methods are prerequisites, never successful observations.
export async function invoke(target, method, ...args) {
  if (typeof target?.[method] !== 'function') throw new Blocked('eg_fixture_method_' + method.toLowerCase());
  return target[method](...args);
}

export async function openBrowser(ctx, actor = 'user', options = {}) {
  const browser = ctx.fixture('browser');
  const session = await invoke(browser, 'open', {actor, am: ctx.am});
  if (typeof session?.close !== 'function') throw Error('browser_cleanup_custody_missing');
  ctx.defer('close_browser', () => session.close());
  ctx.defer('retain_browser_attribution', async () => {
    const value = await ctx.observe('browser_attribution', () => invoke(browser, 'evidence', session.page));
    if (value.real_ui !== true || value.secrets_redacted !== true
      || !/^[0-9a-f]{40}$/.test(value.compiled_source ?? '')
      || !Array.isArray(value.requests) || value.requests.length === 0
      || value.requests.some(r => !r.request_id || !['a', 'b'].includes(r.am))) {
      throw Error('browser_attribution_missing');
    }
  });
  ctx.assert(typeof session.page?.getByRole === 'function', 'playwright_page_required');
  if (Object.keys(options).length || ['EG-002', 'EG-007', 'EG-008'].includes(ctx.row.id)) {
    const extra = await invoke(ctx.fixture('eg_browser'), 'prepare', {session, actor, options, ctx});
    if (typeof extra?.close !== 'function') throw Error('browser_extra_cleanup_custody_missing');
    ctx.defer('cleanup_eg_browser_fixture', () => extra.close());
    Object.assign(session, Object.fromEntries(Object.entries(extra).filter(([key]) => key !== 'close')));
  }
  await session.page.goto(new URL('/event-gateway', session.origin).href);
  return session;
}

export const goto = (page, path) => page.goto(new URL(path, page.url()).href);

export async function account(page) {
  const menu = page.getByRole('menu', {name: 'Account', exact: true});
  if (!await menu.isVisible()) await page.getByRole('button', {name: /^Account menu for /}).click();
  await menu.waitFor({state: 'visible'});
  return menu;
}

export async function service(page, name) {
  await page.getByRole('button', {name: /^Service: /}).click();
  await page.getByRole('menuitemradio', {name, exact: true}).click();
  await page.getByRole('button', {name: `Service: ${name}`, exact: true}).waitFor({state: 'visible'});
}

export async function visible(ctx, locator, code) {
  await locator.waitFor({state: 'visible'});
  ctx.assert(await locator.isVisible(), code);
}

export async function uiReceipt(ctx, page, label = 'browser_receipt') {
  const evidence = await ctx.observe('browser_evidence', () => invoke(ctx.fixture('browser'), 'evidence', page));
  ctx.assert(evidence.real_ui === true && evidence.secrets_redacted === true
    && /^[0-9a-f]{40}$/.test(evidence.compiled_source ?? ''), 'compiled_browser_required');
  const receipt = await ctx.observe(label, async () => ({
    // No rendered identity, input value or browser storage is retained.
    pathname: new URL(page.url()).pathname,
    screenshot_sha256: digest(await page.screenshot()),
  }));
  ctx.assert(/^[0-9a-f]{64}$/.test(receipt.screenshot_sha256), 'browser_receipt_required');
}

export async function acquire(ctx, scenario) {
  const eg = ctx.fixture('eg');
  const lease = await invoke(eg, 'acquire', {
    scenario, caseId: ctx.row.case_id, phase: ctx.phase, iteration: ctx.iteration, am: ctx.am,
  });
  if (typeof lease?.close !== 'function') throw Error('eg_cleanup_custody_missing');
  ctx.defer('cleanup_eg_fixture', () => lease.close());
  ctx.assert(lease.local === true && lease.synthetic === true, 'local_synthetic_fixture_required');
  ctx.assert(Array.isArray(lease.replicas) && lease.replicas.length === 2
    && new Set(lease.replicas.map(r => r.instance)).size === 2
    && lease.replicas.every(r => r.ready === true && /@sha256:[0-9a-f]{64}$/.test(r.image ?? '')),
  'two_real_eg_replicas_required');
  return lease;
}

// EG transport is distinct from ctx.request's AM transport. Adapter responses
// must include their observed EG process identity; no application is launched here.
export async function egRequest(ctx, lease, {method = 'GET', path, body, key, actor = 'owner',
  replica = ctx.am, allowed = [200], ...options}) {
  const response = await invoke(lease, 'request', {method, path, body, key, actor, replica, ...options});
  ctx.assert(allowed.includes(response?.status), 'eg_response_status');
  ctx.assert(lease.replicas.some(r => r.slot === replica && r.instance === response.instance),
    'eg_response_replica_attribution');
  ctx.assert(typeof response.requestId === 'string' && response.requestId.length > 0,
    'eg_request_correlation');
  await ctx.observe('eg_http_receipt', async () => ({method, status: response.status,
    replica, instance_sha256: digest(response.instance), request_sha256: digest(response.requestId),
    body_sha256: digest(response.body ?? null)}));
  return response.body;
}

export function eventJoin(ctx, event, evidence) {
  for (const key of ['eventId', 'conversationId', 'responseId', 'agentId', 'consumerId']) {
    ctx.assert(typeof event[key] === 'string' && event[key].length > 0, 'event_identity_' + key.toLowerCase());
    ctx.assert(evidence[key] === event[key], 'event_join_' + key.toLowerCase());
  }
  ctx.assert(evidence.servingAm !== evidence.owningAm
    && ['a', 'b'].includes(evidence.servingAm) && ['a', 'b'].includes(evidence.owningAm),
  'eg_non_owner_am_ingress');
  ctx.assert(Number.isSafeInteger(evidence.epoch) && evidence.epoch > 0, 'eg_owner_epoch');
  ctx.assert(evidence.providerAccepted === true && evidence.turnCount === 1
    && evidence.effectCount === 1, 'eg_exactly_one_turn_effect');
}

export function delivered(ctx, event, receipt) {
  ctx.assert(receipt.eventId === event.eventId && receipt.responseId === event.responseId,
    'reply_response_identity');
  ctx.assert(receipt.destination === event.destination && receipt.sender === event.expectedSender,
    'reply_sender_destination');
  ctx.assert(receipt.contentDigest === event.expectedContentDigest, 'reply_content');
  ctx.assert(receipt.status === 'delivered' && receipt.count === 1, 'reply_delivered_once');
}
