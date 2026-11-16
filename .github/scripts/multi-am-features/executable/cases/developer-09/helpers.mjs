import {readFileSync} from 'node:fs';
import {isDeepStrictEqual} from 'node:util';
import {Blocked} from '../../runner.mjs';

const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
export const canonicalPath = value => new URL(value, 'http://local.invalid').pathname.replace(/^\/api\/agent-manager\//, '/api/');
export const other = am => am === 'a' ? 'b' : 'a';
export const equal = isDeepStrictEqual;
export const pause = () => new Promise(resolve => setTimeout(resolve, 2000));
export const id = value => {
  if (typeof value !== 'string' || !/^[-a-zA-Z0-9_:.]{1,200}$/.test(value)) throw Error('fixture_identity_invalid');
  return encodeURIComponent(value);
};
export function define(number, fixtures, run, scope = 'browser') {
  const row = catalog.rows.find(row => row.id === `ADM-${String(number).padStart(3, '0')}`);
  if (!row) throw Error('unknown_admin_row');
  return {id: row.case_id, scope, fixtures, steps: row.required_steps, run};
}
export async function take(ctx, name) {
  const fixture = await ctx.fixture('admin-fixtures').take(name, ctx);
  if (!fixture || typeof fixture.cleanup !== 'function') throw new Blocked('admin_setup_missing');
  ctx.defer('release_' + name, () => fixture.cleanup());
  if (fixture.scope !== 'LOCAL_DISPOSABLE_TWO_AM') throw new Blocked('admin_setup_not_disposable');
  return fixture;
}
export async function open(ctx, actor = 'admin', agentId) {
  const browser = ctx.fixture('browser');
  const session = await browser.open({actor, am: ctx.am, agentId});
  ctx.defer('close_and_retain_admin_browser', async () => {
    await session.close();
    const evidence = await browser.evidence(session.page);
    if (!Array.isArray(evidence?.requests) || evidence.requests.some(r => !r.request_id || !['a', 'b'].includes(r.am))) {
      throw Error('admin_browser_attribution_missing');
    }
    // A missing prerequisite can stop before the first request. Preserve BLOCKED;
    // the runner separately rejects a successful case with no observation.
    if (evidence.requests.length) await ctx.observe('admin_browser', async () => evidence);
  });
  return session;
}
export const goto = (session, path) => session.page.goto(new URL(path, session.origin).href);
export async function visible(ctx, locator, code = 'expected_visible') {
  await locator.waitFor({state: 'visible', timeout: 30000});
  ctx.assert(await locator.isVisible(), code);
}
export async function pick(page, label, option) {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name: option, exact: true}).click();
}
export async function poll(ctx, label, read, done, milliseconds = 120000) {
  const until = performance.now() + milliseconds;
  for (;;) {
    const response = await read();
    if (done(response)) { ctx.assert(true, label + '_observed'); return response; }
    ctx.assert(performance.now() < until, label + '_timeout');
    await pause();
  }
}
export async function pages(ctx, path, {actor = 'admin', key = 'id', expected, limit = 2, inspect} = {}) {
  const seen = [], cursors = new Set(); let cursor, index = 0;
  do {
    const query = new URL(path, 'http://local.invalid');
    query.searchParams.set('limit', String(limit));
    if (cursor) query.searchParams.set('cursor', cursor);
    const result = await ctx.request({actor, am: index % 2 ? other(ctx.am) : ctx.am,
      method: 'GET', path: query.pathname + query.search});
    ctx.assert(result.timing?.am === (index % 2 ? other(ctx.am) : ctx.am) && !!result.timing?.request_id, 'page_actual_serving_am');
    ctx.assert(Array.isArray(result.body?.items), 'page_items_missing');
    if (inspect) await inspect(result.body);
    for (const row of result.body.items) {
      ctx.assert(typeof row[key] === 'string' && !seen.includes(row[key]), 'page_identity_duplicate');
      seen.push(row[key]);
    }
    cursor = result.body.next;
    ctx.assert(cursor == null || (typeof cursor === 'string' && !cursors.has(cursor)), 'page_cursor_cycle');
    if (cursor) cursors.add(cursor);
    ctx.assert(++index <= 100, 'pagination_bound');
  } while (cursor);
  if (expected) ctx.assert(equal([...seen].sort(), [...expected].sort()), 'frozen_cohort_mismatch');
  return seen;
}
export function quantity(value) { return value !== null && value !== '' && Number.isFinite(Number(value)); }
export function completeReport(report, expected, principal) {
  if (report?.status !== 'ready' || report.report_complete !== true || report.scope?.admin !== true
    || typeof report.report_id !== 'string' || !report.report_id || !report.summary) return false;
  if (principal && report.scope.principal !== principal) return false;
  for (const key of ['cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours']) {
    const value = report.summary.allocated_period?.[key];
    if (!quantity(value) || Number(value) < 0) return false;
    if (expected && (!quantity(expected.allocated_period?.[key])
      || Math.abs(Number(value) - Number(expected.allocated_period[key])) > 0.000001)) return false;
  }
  return !expected || Number(report.summary.sandbox_count) === expected.sandbox_count;
}
export function captureUsage(session) {
  const rows = [], pending = new Set(), failures = [];
  const listener = response => {
    const url = new URL(response.url()); url.pathname = canonicalPath(response.url());
    if (!url.pathname.startsWith('/api/v1/') || !url.pathname.includes('/usage/')) return;
    const task = (async () => {
      let body = null; try { body = await response.json(); } catch { /* retain transport failure */ }
      const request = response.request();
      let requestBody; try { requestBody = request.postDataJSON(); } catch { /* GET */ }
      rows.push({method: request.method(), path: url.pathname + url.search, status: response.status(),
        request_id: response.headers()['x-request-id'], body, request_body: requestBody,
        observed_ms: performance.now()});
    })();
    pending.add(task); void task.then(() => pending.delete(task), () => {
      failures.push('usage_response_capture_failed'); pending.delete(task);
    });
  };
  session.page.on('response', listener);
  return {rows, async flush() { await Promise.all([...pending]); if (failures.length) throw Error(failures[0]); return rows; },
    async close() { session.page.off('response', listener); await session.close(); await Promise.all([...pending]); if (failures.length) throw Error(failures[0]); }};
}
export function readyFrom(rows, range) {
  const duration = {'1h': 3600, '24h': 86400, '7d': 604800, '30d': 2592000}[range];
  return rows.flatMap(row => [row.body?.report, row.body]).filter(report => report?.status === 'ready'
    && report.scope?.admin === true && (duration === undefined || report.query?.to - report.query?.from === duration)).at(-1);
}
export function matchesSelection(report, range, params) {
  const duration = {'1h': 3600, '24h': 86400, '7d': 604800, '30d': 2592000}[range];
  if (report?.status !== 'ready' || !report.scope?.admin || !report.query?.filters) return false;
  if (duration && Number(report.query.to) - Number(report.query.from) !== duration) return false;
  if (range === 'custom' && (Number(report.query.from) !== Date.parse(params.get('from')) / 1000
    || Number(report.query.to) !== Date.parse(params.get('to')) / 1000)) return false;
  const filters = report.query.filters;
  for (const [queryKey, urlKey] of [['owner', 'owner'], ['sandbox_id', 'sandbox'],
    ['runtime_version', 'runtime'], ['agent_id', 'agent'], ['search', 'search']]) {
    if ((filters[queryKey] || '') !== (params.get(urlKey) || '')) return false;
  }
  return equal(filters.statuses ?? [], params.get('status') ? [params.get('status')] : []);
}
export function selectedReport(rows, range, params) {
  const descriptors = rows.flatMap(row => [row.body?.report, row.body]).filter(report => matchesSelection(report, range, params));
  const inventory = rows.filter(row => row.status === 200 && /\/reports\/[^/]+\/sandboxes(?:\?|$)/.test(row.path)
    && row.body?.report_id).at(-1);
  if (!inventory) return undefined;
  return descriptors.filter(report => report.report_id === inventory.body.report_id).at(-1);
}
export async function committed(ctx, session, range, capture) {
  const dashboard = session.page.locator('[aria-label="Operations resource dashboard"]');
  const table = dashboard.getByRole('table', {name: /Sandboxes and (measured|allocated) usage/});
  return poll(ctx, 'committed_report', async () => {
    await capture.flush();
    const state = await dashboard.getAttribute('data-usage-state');
    const requested = await dashboard.getAttribute('data-usage-requested-range');
    if (state !== 'committed' || requested !== range || !await table.isVisible()) return undefined;
    if (await table.getAttribute('aria-busy') === 'true') return undefined;
    return selectedReport(capture.rows, range, new URL(session.page.url()).searchParams);
  }, report => !!report);
}
