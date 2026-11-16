import {createHash, randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {readFileSync} from 'node:fs';

export const rows = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url))).rows
  .filter(row => row.family === 'api-supplement');
export const otherAM = am => am === 'a' ? 'b' : 'a';
export const segment = value => encodeURIComponent(String(value));
export const key = () => 'api87-' + randomUUID();
export const equal = isDeepStrictEqual;
export const hash = value => createHash('sha256').update(typeof value === 'string' ? value : JSON.stringify(value)).digest('hex');
export function check(value, code) { if (!value) throw Error(code); }
export function caseOf(number, scope, fixtures, run) {
  const id = 'API:API-SUPPLEMENT-' + String(number).padStart(3, '0');
  const row = rows.find(value => value.case_id === id);
  check(row, 'unknown_case');
  return {id, scope, steps: [...row.required_steps], fixtures, run};
}
export async function request(ctx, method, path, body, options = {}) {
  return ctx.request({actor: 'user', am: ctx.am, method, path, body,
    allowed: [200], ...options});
}
// Cleanup cannot use ctx.assert, which is valid only inside a running step.
export async function cleanupRequest(ctx, method, path, body, options = {}) {
  const response = await request(ctx, method, path, body, options);
  check((options.allowed ?? [200]).includes(response.status), 'cleanup_status');
  return response.body;
}
export async function take(ctx, name) {
  const fixture = await ctx.fixture('api-fixtures').take(name, ctx);
  check(fixture && typeof fixture.cleanup === 'function', 'fixture_cleanup_required');
  ctx.defer('cleanup_' + name.replaceAll('-', '_'), () => fixture.cleanup(ctx));
  return fixture;
}
export async function pages(ctx, path, options = {}) {
  const items = [], seen = new Set();
  let cursor;
  for (let count = 0; count < 50; count += 1) {
    const response = await request(ctx, 'GET', path + (path.includes('?') ? '&' : '?')
      + 'limit=20' + (cursor ? '&cursor=' + segment(cursor) : ''), undefined, options);
    ctx.assert(Array.isArray(response.body.items), 'page_items_required');
    items.push(...response.body.items);
    cursor = response.body.next;
    if (cursor == null) return items;
    ctx.assert(typeof cursor === 'string' && cursor.length > 0 && !seen.has(cursor), 'page_cursor_progress');
    seen.add(cursor);
  }
  throw Error('page_budget_exhausted');
}
export function assertProblem(ctx, response, codes) {
  ctx.assert([400, 401, 403, 404, 409, 410, 422].includes(response.status), 'typed_refusal_status');
  ctx.assert(codes.includes(response.body?.code), 'typed_refusal_code');
}
