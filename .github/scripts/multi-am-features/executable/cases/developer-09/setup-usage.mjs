import {requireSetup, check, reserve, request, wait, key, sha} from './setup-common.mjs';
import {cohortTotals, expectedReport} from './allocation-truth.mjs';
import {id} from './helpers.mjs';
export const basicUsageCases = ['010', '011', '012', '013', '014', '026', '027'].map(number => 'C275:ADM-' + number);
async function paged(ctx, path) {
  const rows = [], cursors = new Set(); let cursor;
  do {
    const url = new URL(path, 'http://local.invalid'); url.searchParams.set('limit', '200');
    if (cursor) url.searchParams.set('cursor', cursor);
    const page = await request(ctx, {actor: 'admin', method: 'GET', path: url.pathname + url.search});
    check(Array.isArray(page.items), 'usage_setup_page'); rows.push(...page.items); cursor = page.next;
    check(!cursor || !cursors.has(cursor), 'usage_setup_cursor_cycle'); if (cursor) cursors.add(cursor);
    check(rows.length <= 10000 && cursors.size <= 100, 'usage_setup_page_bound');
  } while (cursor); return rows;
}
export async function prepareUsage(ctx, primitives, internalRoleSetup = false) {
  requireSetup(internalRoleSetup || basicUsageCases.includes(ctx.row.case_id), 'admin09_advanced_usage_recipe_missing');
  const runtime = primitives.usageRuntime, identities = primitives.identities;
  requireSetup(runtime?.sandbox_profile?.id && Number.isSafeInteger(runtime.sandbox_profile.revision)
    && runtime.sandbox_profile.revision > 0 && typeof runtime.finality === 'function', 'admin09_usage_runtime_required');
  for (const actor of ['admin', 'user', 'outsider']) requireSetup(identities?.[actor]?.actor === actor
    && typeof identities[actor].subject === 'string', 'admin09_usage_identity_required');
  requireSetup(identities.user.subject !== identities.outsider.subject, 'admin09_distinct_usage_owners_required');
  const lease = await reserve(ctx, primitives, 'admin09-usage');
  const owned = [], uncertain = new Set(); let cleanupPromise;
  const cleanup = () => cleanupPromise ??= (async () => {
    const failures = [];
    for (const resource of owned) {
      try {
        const path = '/api/v1/sandboxes/' + id(resource.id);
        const current = await request(ctx, {actor: resource.actor, method: 'GET', path, allowed: [200, 404]});
        if (current?.status && !['retiring', 'retired'].includes(current.status)) await request(ctx, {
          actor: resource.actor, method: 'DELETE', path, key: resource.cleanupKey, allowed: [200, 202, 204]});
        await wait(() => ctx.request({actor: resource.actor, method: 'GET', path, allowed: [200, 404]}),
          r => r.status === 404 || r.body.status === 'retired', 'usage_setup_retirement_timeout');
        const finality = await runtime.finality({sandboxId: resource.id});
        check(finality.sandbox_id === resource.id && finality.runtime_absent === true
          && (finality.home_absent === true || finality.home_retained === true), 'usage_setup_native_cleanup');
      } catch (error) {failures.push(error);}
    }
    if (uncertain.size) failures.push(Error('usage_setup_unknown_creation_outcome'));
    if (failures.length) throw new AggregateError(failures, 'usage_setup_cleanup_failed');
    await lease.release();
  })();
  ctx.defer('admin09_usage_public_cleanup', cleanup);
  for (const actor of ['user', 'outsider']) {
    const intent = {actor, am: ctx.am, method: 'POST', path: '/api/v1/sandboxes', key: key(),
      body: {sandbox_profile: runtime.sandbox_profile, name: 'C275 usage ' + key()}, allowed: [201]};
    await lease.retainIntent(intent); uncertain.add(intent.key);
    const keep = async response => {
      if (typeof response?.body?.id !== 'string') return;
      if (!owned.some(r => r.id === response.body.id)) owned.push({id: response.body.id, name: intent.body.name, actor, cleanupKey: key()});
      await lease.retainDocument(response.body.id); uncertain.delete(intent.key);
    };
    let response;
    try { response = await ctx.request(intent); await keep(response); }
    catch (error) {await keep(error.response); throw error;}
    const resource = owned.at(-1); check(resource?.id === response.body.id, 'usage_setup_custody_identity');
    const path = '/api/v1/sandboxes/' + id(resource.id);
    await wait(() => request(ctx, {actor, method: 'GET', path}), value => {
      check(!['failed', 'retired'].includes(value.status), 'usage_setup_failed'); return value.status === 'active';
    }, 'usage_setup_active_timeout');
    await request(ctx, {actor, method: 'POST', path: path + '/pause', key: key(), allowed: [202]});
    await wait(() => request(ctx, {actor, method: 'GET', path}), value => value.status === 'paused', 'usage_setup_pause_timeout');
  }
  // The exclusive parent lease freezes lifecycle mutations. Expected quantities
  // come from the ordinary lifecycle ledger, never a metrics report response.
  const records = await paged(ctx, '/api/v1/admin/usage/sandboxes');
  requireSetup(records.length >= 2 && records.length <= 100, 'usage_setup_bounded_cohort_required');
  for (const record of records) {
    requireSetup(record.complete_lifecycle === true && !record.allocation.running, 'usage_setup_stable_complete_cohort_required');
    record.events = await paged(ctx, `/api/v1/usage/sandboxes/${id(record.sandbox_id)}/events`);
  }
  const end = Math.floor(Date.now() / 60000) * 60, from = end - 3600;
  const localDate = seconds => {const date = new Date(seconds * 1000); return new Date(date.getTime() - date.getTimezoneOffset() * 60000).toISOString().slice(0, 16);};
  const owners = [...new Set(records.map(r => r.owner))];
  const query = {from: end - 86400, to: end, filters: {}};
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', cleanup, principal: identities.admin.subject,
    sandbox_ids: records.map(r => r.sandbox_id), owned_resources: owned,
    report_query: query, report_expected: expectedReport(records, query),
    expectedReport: query => expectedReport(records, query),
    custom: {from: new Date(from * 1000).toISOString(), to: new Date(end * 1000).toISOString(), from_local: localDate(from), to_local: localDate(end)},
    owner_searches: [{query: '', expected_ids: owners}, {query: key(), expected_ids: []}],
    legacy_truth: [{sandbox_ids: records.map(r => r.sandbox_id)}, ...owners.map(owner => ({owner, sandbox_ids: records.filter(r => r.owner === owner).map(r => r.sandbox_id)}))],
    legacyTotalsAt: (asOf, owner, sandboxId) => cohortTotals(records, 0, asOf, owner, sandboxId),
    private_markers: owned.map(r => r.name)};
  await ctx.observe('usage_setup_interval_truth', async () => ({cohort_sha256: sha(records), sandbox_count: records.length,
    source: 'public_lifecycle_intervals', owned_count: owned.length, actual_api_requests_retained: true}));
  return f;
}
