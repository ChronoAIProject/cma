import {createHash, randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {readFileSync} from 'node:fs';
import {Blocked} from '../../runner.mjs';

export const rows = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url))).rows
  .filter(row => row.family === 'api-supplement');
const fixtureSpecifications = JSON.parse(readFileSync(new URL('./fixtures.json', import.meta.url))).setups;
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
  return ctx.request({actor: 'user', am: options.agentId ? undefined : ctx.am, method, path, body,
    allowed: [200], ...options});
}
// Cleanup cannot use ctx.assert, which is valid only inside a running step.
export async function cleanupRequest(ctx, method, path, body, options = {}) {
  const response = await request(ctx, method, path, body, options);
  check((options.allowed ?? [200]).includes(response.status), 'cleanup_status');
  return response.body;
}
export async function take(ctx, name) {
  const adapter = ctx.fixture('api-fixtures');
  if (typeof adapter.take !== 'function') throw new Blocked('fixture_take_unavailable');
  const fixture = await adapter.take(name, ctx);
  if (!fixture) throw new Blocked('fixture_unavailable_' + name.replaceAll('-', '_'));
  check(typeof fixture.cleanup === 'function', 'fixture_cleanup_required');
  ctx.defer('cleanup_' + name.replaceAll('-', '_'), () => fixture.cleanup(ctx));
  const specification = fixtureSpecifications[name];
  check(specification, 'unknown_named_fixture');
  if (specification.required_methods.some(method => typeof fixture[method] !== 'function')) {
    throw new Blocked('fixture_method_unavailable_' + name.replaceAll('-', '_'));
  }
  if (specification.required_fields.some(field => fixture[field] === undefined)) {
    throw new Blocked('fixture_data_unavailable_' + name.replaceAll('-', '_'));
  }
  return fixture;
}
export async function readOtherAM(ctx, fixture, query, previous) {
  ctx.assert(['a','b'].includes(previous?.timing?.am), 'write_ingress_attribution_required');
  // Shared runner f5d2d08d3 honors explicit ingress and still records live
  // owner/epoch evidence for the read; this never relies on requested labels.
  const response = await ctx.request({...query, am: otherAM(previous.timing.am)});
  ctx.assert(['a','b'].includes(response?.timing?.am) && response.timing.am !== previous.timing.am,
    'cross_am_read_distinct_ingress');
  await ctx.observe('cross_am_read_attribution', async () => ({write_am: previous.timing.am,
    read_am: response.timing.am, request_id: response.timing.request_id}));
  return response;
}
export async function pages(ctx, path, options = {}) {
  const {nextField = 'next', cursorField = 'cursor', ...requestOptions} = options;
  const items = [], seen = new Set();
  let cursor;
  for (let count = 0; count < 50; count += 1) {
    const response = await request(ctx, 'GET', path + (path.includes('?') ? '&' : '?')
      + 'limit=20' + (cursor ? '&' + cursorField + '=' + segment(cursor) : ''), undefined, requestOptions);
    ctx.assert(Array.isArray(response.body.items), 'page_items_required');
    items.push(...response.body.items);
    cursor = response.body[nextField];
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
