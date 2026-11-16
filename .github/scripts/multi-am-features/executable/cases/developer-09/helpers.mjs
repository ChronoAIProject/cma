import {readFileSync} from 'node:fs';
import {isDeepStrictEqual} from 'node:util';
import {Blocked} from '../../runner.mjs';

const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
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
  ctx.defer('close_admin_browser', () => session.close());
  ctx.defer('retain_admin_browser', async () => {
    const evidence = await ctx.observe('admin_browser', () => browser.evidence(session.page));
    if (!Array.isArray(evidence?.requests) || !evidence.requests.length
      || evidence.requests.some(r => !r.request_id || !['a', 'b'].includes(r.am))) {
      throw Error('admin_browser_attribution_missing');
    }
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
    if (done(response)) return response;
    ctx.assert(performance.now() < until, label + '_timeout');
    await pause();
  }
}
export async function pages(ctx, path, {actor = 'admin', key = 'id', expected, limit = 2} = {}) {
  const seen = [], cursors = new Set(); let cursor, index = 0;
  do {
    const query = new URL(path, 'http://local.invalid');
    query.searchParams.set('limit', String(limit));
    if (cursor) query.searchParams.set('cursor', cursor);
    const result = await ctx.request({actor, am: index % 2 ? other(ctx.am) : ctx.am,
      method: 'GET', path: query.pathname + query.search});
    ctx.assert(Array.isArray(result.body?.items), 'page_items_missing');
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
  const rows = [], pending = new Set();
  const listener = response => {
    const url = new URL(response.url());
    if (!url.pathname.startsWith('/api/v1/') || !url.pathname.includes('/usage/')) return;
    const task = (async () => {
      let body = null; try { body = await response.json(); } catch { /* retain transport failure */ }
      const request = response.request();
      let requestBody; try { requestBody = request.postDataJSON(); } catch { /* GET */ }
      rows.push({method: request.method(), path: url.pathname + url.search, status: response.status(),
        request_id: response.headers()['x-request-id'], body, request_body: requestBody,
        observed_ms: performance.now()});
    })();
    pending.add(task); task.finally(() => pending.delete(task));
  };
  session.page.on('response', listener);
  return {rows, async flush() { await Promise.all([...pending]); return rows; },
    async close() { session.page.off('response', listener); await Promise.all([...pending]); }};
}
export function readyFrom(rows, range) {
  const duration = {'1h': 3600, '24h': 86400, '7d': 604800, '30d': 2592000}[range];
  return rows.flatMap(row => [row.body?.report, row.body]).filter(report => report?.status === 'ready'
    && report.scope?.admin === true && (duration === undefined || report.query?.to - report.query?.from === duration)).at(-1);
}
export async function committed(ctx, session, range, capture) {
  const dashboard = session.page.locator('[aria-label="Operations resource dashboard"]');
  await session.page.waitForFunction(({range}) => {
    const node = document.querySelector('[aria-label="Operations resource dashboard"]');
    return node?.getAttribute('data-usage-state') === 'committed'
      && node.getAttribute('data-usage-requested-range') === range;
  }, {range}, {timeout: 120000, polling: 2000});
  ctx.assert(await dashboard.getAttribute('data-usage-state') === 'committed', 'report_not_committed');
  await visible(ctx, dashboard.getByRole('table'), 'report_table_missing');
  await capture.flush();
  const report = readyFrom(capture.rows, range);
  ctx.assert(!!report, 'committed_descriptor_missing');
  return report;
}
