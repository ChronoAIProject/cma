import {caseOf, equal, otherAM, request, segment, take} from './common.mjs';

const quantityNames = ['uptime_seconds', 'cpu_millis_seconds', 'memory_mib_seconds', 'disk_gib_seconds',
  'cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours'];
export function totalsFor(intervals) {
  const result = Object.fromEntries(quantityNames.map(name => [name, 0]));
  for (const interval of intervals) {
    const seconds = interval.ended_at - interval.started_at;
    if (!Number.isSafeInteger(seconds) || seconds < 0 || typeof interval.running !== 'boolean'
      || ['cpu_millis','memory_mib','disk_gib'].some(name=>!Number.isSafeInteger(interval[name]) || interval[name]<0)) {
      throw Error('usage_fixture_interval');
    }
    result.uptime_seconds += interval.running ? seconds : 0;
    result.cpu_millis_seconds += seconds * interval.cpu_millis;
    result.memory_mib_seconds += seconds * interval.memory_mib;
    result.disk_gib_seconds += seconds * interval.disk_gib;
  }
  result.cpu_core_hours = result.cpu_millis_seconds / 3600000;
  result.memory_gib_hours = result.memory_mib_seconds / 3686400;
  result.disk_gib_hours = result.disk_gib_seconds / 3600;
  return result;
}
export function assertTotals(ctx, observed, expected) {
  for (const name of quantityNames) ctx.assert(typeof observed?.[name] === 'number'
    && Number.isFinite(observed[name]) && Math.abs(observed[name] - expected[name]) <= Math.max(1e-9, Math.abs(expected[name]) * 1e-9),
  'usage_' + name);
}
async function readAll(ctx, am, actor = 'user') {
  let cursor;
  const seen = new Set(), values = [];
  for (let page = 0; page < 50; page++) {
    const reply = await request(ctx, 'GET', '/api/v1/usage/sandboxes?limit=1' + (cursor ? '&cursor=' + segment(cursor) : ''),
      undefined, {am, actor});
    ctx.assert(Array.isArray(reply.body.items) && reply.body.items.length <= 1, 'usage_bounded_page');
    for (const item of reply.body.items) {
      ctx.assert(!seen.has(item.sandbox_id), 'usage_no_duplicate_sandbox');
      seen.add(item.sandbox_id); values.push(item);
    }
    const next = reply.body.next;
    if (next === null) return values;
    ctx.assert(typeof next === 'string' && next.length > 0 && next !== cursor, 'usage_cursor_progress');
    cursor = next;
  }
  throw Error('usage_page_budget');
}
function assertSandbox(ctx, observed, seed) {
  ctx.assert(observed.sandbox_id === seed.sandbox_id && observed.owner === seed.owner
    && observed.status === seed.status && observed.name === seed.name, 'usage_resource_identity');
  ctx.assert(equal(observed.allocation, seed.allocation) && observed.allocation.running === false,
    'usage_closed_allocation');
  ctx.assert(['cpu_millis','memory_mib','disk_gib'].every(name=>observed.allocation[name] === 0)
    && observed.as_of >= Math.max(...seed.intervals.map(value=>value.ended_at)), 'usage_settled_measurement_boundary');
  ctx.assert(observed.complete_lifecycle === seed.complete_lifecycle
    && observed.created_at === seed.created_at && observed.tracking_started_at === seed.tracking_started_at,
  'usage_lifecycle_completeness');
  assertTotals(ctx, observed.totals, totalsFor(seed.intervals));
}
function assertSummary(ctx, summary, ledger) {
  ctx.assert(summary.measurement === 'allocated_resource_time' && summary.sandbox_count === ledger.length
    && summary.running_count === 0 && summary.incomplete_lifecycle_count === ledger.filter(s => !s.complete_lifecycle).length,
  'usage_summary_denominator');
  assertTotals(ctx, summary.totals, totalsFor(ledger.flatMap(s => s.intervals)));
}
export const usageCases = [69,70,71].map(number => caseOf(number, 'global', ['api-fixtures'], async ctx => {
  let fixture, initial, summary;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'usage');
    ctx.assert(fixture.ledger?.length >= 2, 'usage_multiple_known_sandboxes');
    summary = (await request(ctx, 'GET', '/api/v1/usage')).body;
    assertSummary(ctx, summary, fixture.ledger);
    initial = await readAll(ctx, ctx.am);
    ctx.assert(initial.length === fixture.ledger.length, 'usage_no_missing_sandbox');
    for (const seed of fixture.ledger) {
      const observed = initial.find(value => value.sandbox_id === seed.sandbox_id);
      ctx.assert(Boolean(observed), 'usage_seed_listed');
      assertSandbox(ctx, observed, seed);
      const point = await request(ctx, 'GET', '/api/v1/usage/sandboxes/' + segment(seed.sandbox_id));
      assertSandbox(ctx, point.body, seed);
    }
  });
  await ctx.step('step_2', async () => {
    const path = ctx.row.entry_point.split(' ')[1].replace('{id}', segment(fixture.ledger[0].sandbox_id));
    const reply = await request(ctx, 'GET', path);
    if (number === 69) assertSummary(ctx, reply.body, fixture.ledger);
    else if (number === 71) assertSandbox(ctx, reply.body, fixture.ledger[0]);
    else {
      ctx.assert(Array.isArray(reply.body.items) && reply.body.items.length > 0, 'usage_exact_page');
      for (const item of reply.body.items) {
        const seed = fixture.ledger.find(s => s.sandbox_id === item.sandbox_id);
        ctx.assert(Boolean(seed), 'usage_no_foreign_sandbox'); assertSandbox(ctx, item, seed);
      }
    }
  });
  await ctx.step('step_3', async () => {
    const peer = await readAll(ctx, otherAM(ctx.am));
    ctx.assert(peer.length === initial.length, 'usage_cross_am_page_count');
    for (const seed of fixture.ledger) {
      const observed = peer.find(value => value.sandbox_id === seed.sandbox_id);
      ctx.assert(Boolean(observed), 'usage_cross_am_seed'); assertSandbox(ctx, observed, seed);
      const denied = await request(ctx, 'GET', '/api/v1/usage/sandboxes/' + segment(seed.sandbox_id),
        undefined, {actor: 'outsider', am: otherAM(ctx.am), allowed: [403,404]});
      ctx.assert(typeof denied.body.code === 'string' && !('totals' in denied.body), 'usage_other_owner_denied');
    }
    const outsiders = await readAll(ctx, otherAM(ctx.am), 'outsider');
    ctx.assert(outsiders.every(value => !fixture.ledger.some(seed => seed.sandbox_id === value.sandbox_id)), 'usage_owner_list_isolation');
  });
  await ctx.step('expected_result', async () => {
    const final = (await request(ctx, 'GET', '/api/v1/usage', undefined, {am: otherAM(ctx.am)})).body;
    assertSummary(ctx, final, fixture.ledger);
    ctx.assert(equal(final.totals, summary.totals), 'usage_stable_closed_totals');
    await ctx.observe('usage_known_allocation_inputs', async () => ({sandbox_count: fixture.ledger.length,
      interval_count: fixture.ledger.flatMap(s => s.intervals).length, measurement: final.measurement}));
  });
}));
