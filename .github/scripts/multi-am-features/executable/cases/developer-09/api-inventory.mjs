import {define, take, other, id, pages, equal, quantity, poll, completeReport} from './helpers.mjs';
const P = '/api/v1/admin';
export async function createReport(ctx, f) {
  const query = f.report_query;
  ctx.assert(Number.isSafeInteger(query?.from) && Number.isSafeInteger(query?.to), 'usage_query_fixture');
  const made = await ctx.request({actor: 'admin', method: 'POST', path: P + '/usage/metrics/reports', body: query, allowed: [200, 202]});
  const reportId = made.body.report_id;
  const result = await poll(ctx, 'report_ready', async () => ctx.request({actor: 'admin', am: other(ctx.am),
    method: 'GET', path: P + '/usage/metrics/reports/' + id(reportId)}), response => {
    ctx.assert(response.body.report_id === reportId && response.body.status !== 'failed', 'report_identity_or_failure');
    return response.body.status === 'ready';
  });
  ctx.assert(completeReport(result.body, f.report_expected, f.principal), 'report_totals_fixture');
  return result.body;
}
export const cases = [define(26, ['admin-fixtures'], async ctx => {
  const f = await take(ctx, 'usage'); let report, owners;
  await ctx.step('step_1', async () => { report = await createReport(ctx, f); });
  await ctx.step('step_2', async () => {
    const path = `${P}/usage/metrics/reports/${id(report.report_id)}/owners`;
    for (const search of f.owner_searches) {
      owners = await pages(ctx, path + '?search=' + encodeURIComponent(search.query), {key: 'subject', expected: search.expected_ids, inspect: body => ctx.assert(body.report_id === report.report_id, 'owner_page_wrong_report')});
      ctx.assert(owners.length === search.expected_ids.length, 'owner_search_cohort');
    }
    const denied = await ctx.request({actor: 'user', am: other(ctx.am), method: 'GET', path, allowed: [403, 404]});
    ctx.assert(!JSON.stringify(denied.body).includes(report.report_id), 'report_owner_scope_disclosed');
  });
  await ctx.step('expected_result', async () => {
    const seen = await ctx.request({actor: 'admin', method: 'GET', path: `${P}/usage/metrics/reports/${id(report.report_id)}/owners?limit=1`});
    ctx.assert(seen.body.report_id === report.report_id && seen.body.items.length <= 1, 'owner_page_binding');
    ctx.assert(Array.isArray(owners), 'owners_not_exercised');
  });
}, 'global'), define(27, ['admin-fixtures'], async ctx => {
  const f = await take(ctx, 'usage'); const summaries = [];
  await ctx.step('step_1', async () => {
    for (const selection of f.legacy_truth) {
      const path = P + '/usage' + (selection.owner ? '?owner=' + encodeURIComponent(selection.owner) : '');
      const result = await ctx.request({actor: 'admin', method: 'GET', path});
      const value = result.body;
      ctx.assert(value.measurement === 'allocated_resource_time' && Number.isSafeInteger(value.as_of), 'legacy_measurement_scope');
      ctx.assert(value.sandbox_count === selection.sandbox_ids.length, 'legacy_count');
      // Expected values use the fixture's allocation intervals at this returned as_of.
      const expected = await f.legacyTotalsAt(value.as_of, selection.owner);
      ctx.assert(equal(value.totals, expected), 'legacy_interval_totals');
      summaries.push({selection, value});
    }
  });
  await ctx.step('step_2', async () => {
    for (const {selection, value} of summaries) {
      const path = P + '/usage/sandboxes' + (selection.owner ? '?owner=' + encodeURIComponent(selection.owner) : '');
      const rows = await pages(ctx, path, {key: 'sandbox_id', expected: selection.sandbox_ids, inspect: async body => {
        ctx.assert(Number.isSafeInteger(body.as_of), 'legacy_page_as_of');
        for (const row of body.items) {
          const truth = await f.legacyTotalsAt(row.as_of ?? body.as_of, selection.owner, row.sandbox_id);
          ctx.assert(equal(row.totals, truth), 'legacy_inventory_interval_totals');
        }
      }});
      ctx.assert(rows.length === value.sandbox_count, 'legacy_summary_inventory_count');
    }
  });
  await ctx.step('expected_result', async () => {
    for (const path of ['/usage', '/usage/sandboxes']) {
      const denied = await ctx.request({actor: 'user', method: 'GET', path: P + path, allowed: [403]});
      ctx.assert(denied.status === 403, 'legacy_usage_requires_admin');
    }
    ctx.assert(summaries.length >= 2 && summaries.every(s => Object.values(s.value.totals).every(quantity)), 'legacy_totals_missing');
  });
}, 'global'), define(30, ['admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); let original;
  await ctx.step('step_1', async () => {
    ctx.assert(f.inventory_filters.length >= 4, 'all_inventory_filter_kinds_required');
    for (const filter of f.inventory_filters) await pages(ctx, P + '/sandboxes?' + new URLSearchParams(filter.query), {actor: 'read_operator', expected: filter.expected_ids});
  });
  await ctx.step('step_2', async () => {
    original = (await ctx.request({actor: 'read_operator', method: 'GET', path: P + '/sandboxes/' + id(f.sandbox_id)})).body;
    const peer = (await ctx.request({actor: 'read_operator', am: other(ctx.am), method: 'GET', path: P + '/sandboxes/' + id(f.sandbox_id)})).body;
    ctx.assert(original.sandbox.id === f.sandbox_id && equal(peer.sandbox, original.sandbox), 'inventory_original_identity');
  });
  await ctx.step('expected_result', async () => {
    await ctx.request({actor: 'read_operator', method: 'POST', path: P + '/sandboxes/' + id(f.sandbox_id) + '/retire', allowed: [403]});
    const current = (await ctx.request({actor: 'read_operator', am: other(ctx.am), method: 'GET', path: P + '/sandboxes/' + id(f.sandbox_id)})).body;
    ctx.assert(equal(current.sandbox, original.sandbox), 'denied_inventory_write_changed_state');
    ctx.assert(f.private_markers.every(m => !JSON.stringify(current).includes(m)), 'inventory_private_data');
  });
}, 'global'), define(37, ['admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); const observed = [];
  await ctx.step('step_1', async () => {
    for (const am of [ctx.am, other(ctx.am)]) {
      const result = await ctx.request({actor: 'runtime_operator', am, method: 'GET', path: P + '/runtime-inventory'});
      ctx.assert(Number.isFinite(Date.parse(result.body.observed_at)) && Array.isArray(result.body.runtimes), 'runtime_inventory_shape');
      observed.push(result.body);
    }
  });
  await ctx.step('step_2', async () => {
    const provider = await ctx.observe('native_runtime_inventory', () => f.observeProvider());
    ctx.assert(['starting', 'ready', 'unavailable'].every(status => provider.runtimes.some(r => r.status === status)), 'runtime_observer_status_fixture');
    for (const view of observed) {
      for (const runtime of provider.runtimes) {
        const row = view.runtimes.find(x => x.runtime_id === runtime.runtime_id);
        ctx.assert(row?.status === runtime.status && row.generation === runtime.generation, 'runtime_observer_mismatch');
      }
    }
  });
  await ctx.step('expected_result', async () => {
    await ctx.request({actor: 'read_operator', method: 'GET', path: P + '/runtime-inventory', allowed: [403]});
    await ctx.request({actor: 'runtime_operator', method: 'GET', path: P + '/sandboxes', allowed: [403]});
    ctx.assert(observed.every(x => x.runtimes.every(r => ['starting', 'ready', 'unavailable'].includes(r.status))), 'runtime_unknown_claimed_ready');
  });
}, 'global'), define(38, ['admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); let seen = [];
  await ctx.step('step_1', async () => {
    ctx.assert(f.profile_filters.length >= 3, 'profile_kinds_required');
    for (const filter of f.profile_filters) seen.push(...await pages(ctx, P + '/profiles?' + new URLSearchParams(filter.query), {actor: 'read_operator', expected: filter.expected_ids}));
  });
  await ctx.step('step_2', async () => {
    for (const profile of f.profiles) {
      const page = await ctx.request({actor: 'read_operator', am: other(ctx.am), method: 'GET',
        path: P + '/profiles?' + new URLSearchParams({kind: profile.kind, owner: profile.owner, limit: '200'})});
      const entry = page.body.items.find(x => x.id === profile.id);
      ctx.assert(entry?.owner === profile.owner && typeof entry.name === 'string' && entry.kind === profile.kind, 'profile_summary_identity');
      ctx.assert(seen.includes(profile.id), 'profile_missing_from_inventory');
      const revision = await ctx.request({actor: profile.actor, am: other(ctx.am), method: 'GET',
        path: `/api/v1/${profile.kind}-profiles/${id(profile.id)}/revisions/${profile.revision}`});
      ctx.assert(revision.body.profile_id === profile.id && revision.body.revision === profile.revision, 'profile_revision_identity');
    }
  });
  await ctx.step('expected_result', async () => {
    for (const profile of f.profiles) {
      const denied = await ctx.request({actor: 'read_operator', method: 'POST',
        path: P + '/profiles/' + profile.kind + '/' + id(profile.id) + '/retire', allowed: [403]});
      ctx.assert(denied.status === 403, 'profile_inventory_granted_control');
      await ctx.request({actor: 'read_operator', method: 'PUT', path: `/api/v1/${profile.kind}-profiles/${id(profile.id)}`,
        key: `denied-author-${profile.id}`, body: profile.update_body, allowed: [403, 404]});
      const original = await ctx.request({actor: profile.actor, method: 'GET', path: `/api/v1/${profile.kind}-profiles/${id(profile.id)}`});
      ctx.assert(equal(original.body.draft, profile.original_draft), 'denied_author_changed_draft');
    }
  });
}, 'global')];
