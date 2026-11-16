import {define, take, open, goto, visible, other, id, equal, poll, pages} from './helpers.mjs';
const P = '/api/v1/admin/sandboxes/';
const path = f => P + id(f.sandbox_id);
const read = (ctx, f, am = other(ctx.am)) => ctx.request({actor: 'admin', am, method: 'GET', path: path(f)});
async function details(ctx, f, actor = 'admin') {
  const session = await open(ctx, actor); await goto(session, '/operations');
  await session.page.getByRole('button', {name: `Runtime details for ${f.sandbox_name}`, exact: true}).click();
  const modal = session.page.getByRole('dialog', {name: 'Sandbox details', exact: true});
  await visible(ctx, modal); return {session, modal};
}
async function retireDialog(ctx, f) {
  const session = await open(ctx); await goto(session, '/operations');
  const row = session.page.getByRole('row').filter({has: session.page.getByRole('button', {name: f.sandbox_name, exact: true})});
  await row.getByRole('button', {name: 'Retire', exact: true}).click();
  const modal = session.page.getByRole('dialog', {name: `Retire ${f.sandbox_name}`, exact: true});
  await visible(ctx, modal);
  ctx.assert(await modal.getByRole('button', {name: 'Retire Sandbox', exact: true}).isDisabled(), 'retire_requires_ack');
  await modal.getByLabel('I understand this Sandbox cannot be resumed.', {exact: true}).check();
  return {session, modal};
}
async function finalRetirement(ctx, f) {
  await poll(ctx, 'retirement', async () => ctx.request({actor: 'admin', am: other(ctx.am), method: 'GET', path: path(f), allowed: [200, 404]}),
    r => r.status === 404 || r.body.sandbox.status === 'retired');
  const native = await poll(ctx, 'native_retirement', () => f.observeProvider(), r =>
    r.runtime_absent === true && (r.home_absent === true || (r.home_retained === true && r.home_identity === f.home_identity)));
  await ctx.observe('retirement_finality', async () => native);
  ctx.assert(native.runtime_absent, 'retirement_acceptance_is_not_finality');
}
function eventSequence(ctx, rows, sandbox, after = 0) {
  let previous = after;
  for (const event of rows) {
    ctx.assert(event.sandbox_id === sandbox && Number.isSafeInteger(event.seq) && event.seq > previous, 'event_sequence_or_scope');
    ctx.assert(Number.isFinite(Date.parse(event.appended_at)), 'event_time'); previous = event.seq;
  }
}
export const cases = [define(31, ['browser', 'admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); let ui, original;
  await ctx.step('step_1', async () => {
    ui = await details(ctx, f); original = (await read(ctx, f, ctx.am)).body;
    ctx.assert(original.sandbox.id === f.sandbox_id && original.sandbox.profile.revision === f.profile_revision, 'sandbox_revision_identity');
    for (const value of [f.sandbox_id, f.owner_name, original.sandbox.status.replaceAll('_', ' '),
      original.sandbox.runtime?.runtime_version ?? 'not bound', `revision ${f.profile_revision}`])
      ctx.assert((await ui.modal.innerText()).includes(value), 'sandbox_detail_field');
    ctx.assert(await ui.modal.locator('time').filter({hasNotText: ''}).count() > 0, 'sandbox_created_time');
  });
  await ctx.step('step_2', async () => {
    for (const workspace of original.workspaces) ctx.assert((await ui.modal.innerText()).includes(workspace.name), 'workspace_hierarchy');
    for (const agent of original.agents) {
      ctx.assert(original.workspaces.some(w => w.id === agent.workspace_id), 'agent_wrong_workspace');
      ctx.assert((await ui.modal.innerText()).includes(agent.title?.trim() || 'Untitled agent'), 'agent_hierarchy');
    }
    await ui.session.switchAM(other(ctx.am));
    const response = ui.session.page.waitForResponse(r => new URL(r.url()).pathname === path(f) && r.request().method() === 'GET');
    await ui.modal.getByRole('button', {name: 'Refresh details', exact: true}).click();
    const refreshed = await (await response).json();
    ctx.assert(refreshed.sandbox.id === f.sandbox_id && equal(refreshed.workspaces, original.workspaces)
      && equal(refreshed.agents, original.agents), 'refreshed_hierarchy');
  });
  await ctx.step('expected_result', async () => {
    const text = await ui.modal.innerText();
    ctx.assert(f.private_markers.every(marker => !(JSON.stringify(original) + text).includes(marker)), 'hierarchy_private_data');
  });
}), define(32, ['browser', 'admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); let ui, first, second;
  await ctx.step('step_1', async () => {
    ui = await details(ctx, f);
    first = (await ctx.request({actor: 'admin', method: 'GET', path: path(f) + '/events?limit=50'})).body;
    ctx.assert(first.length === 50, 'event_pagination_fixture'); eventSequence(ctx, first, f.sandbox_id);
    const response = ui.session.page.waitForResponse(r => new URL(r.url()).pathname === path(f) + '/events' && new URL(r.url()).searchParams.get('after') === String(first.at(-1).seq));
    await ui.session.switchAM(other(ctx.am));
    await ui.modal.getByRole('button', {name: 'Next events', exact: true}).click();
    second = await (await response).json(); eventSequence(ctx, second, f.sandbox_id, first.at(-1).seq);
    await ui.modal.getByRole('button', {name: 'Refresh details', exact: true}).click();
    await visible(ctx, ui.modal.getByRole('button', {name: 'First events', exact: true}));
  });
  await ctx.step('step_2', async () => {
    const all = [...first, ...second];
    ctx.assert(new Set(all.map(e => e.seq)).size === all.length, 'event_duplicates');
    for (const event of f.events) ctx.assert(all.some(e => equal(e, event)), 'fixture_event_missing');
    ctx.assert(all.some(e => e.kind.failure) && all.some(e => e.kind.from), 'failure_transition_fixture');
    for (const event of second) {
      if (event.kind.failure) ctx.assert((await ui.modal.innerText()).includes(event.kind.failure.detail), 'failure_detail_not_rendered');
      if (event.kind.from) ctx.assert((await ui.modal.innerText()).includes(`${event.kind.from.replaceAll('_', ' ')} to ${event.kind.to.replaceAll('_', ' ')}`), 'transition_not_rendered');
    }
  });
  await ctx.step('expected_result', async () => {
    await ui.modal.getByRole('button', {name: 'First events', exact: true}).click();
    const result = await ctx.request({actor: 'admin', am: other(ctx.am), method: 'GET', path: path(f) + '/events?limit=50'});
    ctx.assert(equal(result.body, first), 'event_first_page_changed');
  });
}), define(33, ['browser', 'admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); let ui, initial;
  await ctx.step('step_1', async () => {
    initial = await ctx.observe('native_before_retire', () => f.observeProvider());
    ctx.assert(initial.runtime_id && initial.home_identity === f.home_identity, 'retire_native_fixture');
    await ctx.request({actor: 'read_operator', method: 'POST', path: path(f) + '/retire', allowed: [403]});
    ctx.assert((await read(ctx, f)).body.sandbox.owner === f.other_owner, 'retirement_cross_owner_fixture');
  });
  await ctx.step('step_2', async () => {
    ui = await retireDialog(ctx, f);
    const fault = await take(ctx, 'usage_faults');
    const hold = await fault.arm({kind: 'hold_response', method: 'POST', path: path(f) + '/retire'});
    ctx.defer('retire_hold', () => hold.close());
    await ui.modal.getByRole('button', {name: 'Retire Sandbox', exact: true}).click();
    await poll(ctx, 'retire_accepted', () => hold.observations(), rows => rows.some(r => r.status === 200));
    ctx.assert(await ui.modal.getByRole('button', {name: 'Retire Sandbox', exact: true}).isDisabled(), 'retire_double_submit_enabled');
    await hold.release();
    const posts = (await hold.observations()).filter(r => r.method === 'POST');
    ctx.assert(posts.length === 1 && ['retiring', 'retired'].includes(posts[0].body.status), 'retire_single_acceptance');
  });
  await ctx.step('step_3', async () => { await finalRetirement(ctx, f); });
  await ctx.step('expected_result', async () => {
    await goto(ui.session, '/operations');
    const row = ui.session.page.getByRole('row').filter({has: ui.session.page.getByRole('button', {name: f.sandbox_name, exact: true})});
    if (await row.count()) ctx.assert(await row.getByRole('button', {name: 'Retire', exact: true}).isDisabled(), 'retired_row_enabled');
    ctx.assert((await f.audit({target: f.sandbox_id, action: 'delete'})).filter(r => r.outcome.status === 'succeeded').length === 1, 'retire_audit_count');
  });
}), define(34, ['browser', 'admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'), faults = await take(ctx, 'usage_faults'); let ui, drop;
  await ctx.step('step_1', async () => {
    ui = await retireDialog(ctx, f); drop = await faults.arm({kind: 'drop_accepted_response', method: 'POST', path: path(f) + '/retire'});
    ctx.defer('retire_drop', () => drop.close());
    await ui.modal.getByRole('button', {name: 'Retire Sandbox', exact: true}).click();
    const accepted = await poll(ctx, 'retire_upstream', () => drop.observations(), rows => rows.some(r => r.status === 200));
    ctx.assert(accepted.filter(r => r.method === 'POST').length === 1, 'retire_upstream_duplicate');
  });
  await ctx.step('step_2', async () => { await visible(ctx, ui.modal.getByText('The previous outcome is unknown. Only a read will be sent next.', {exact: true})); });
  await ctx.step('step_3', async () => {
    const next = ui.session.page.waitForRequest(r => new URL(r.url()).pathname.startsWith(path(f)));
    await ui.session.switchAM(other(ctx.am));
    await ui.modal.getByLabel('I understand this Sandbox cannot be resumed.', {exact: true}).check();
    await ui.modal.getByRole('button', {name: 'Check current state', exact: true}).click();
    const request = await next;
    ctx.assert(request.method() === 'GET' && new URL(request.url()).pathname === path(f), 'uncertain_retire_replayed');
    await ui.modal.waitFor({state: 'hidden', timeout: 30000}); ctx.assert(!await ui.modal.isVisible(), 'retire_reconcile_not_closed');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert((await drop.observations()).filter(r => r.method === 'POST').length === 1, 'uncertain_retire_duplicate_effect');
    await finalRetirement(ctx, f);
  });
}), define(35, ['browser', 'admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); let ui, original, latest;
  await ctx.step('step_1', async () => {
    original = (await read(ctx, f)).body.sandbox; ui = await details(ctx, f);
    ctx.assert(original.profile.always_on !== true && original.revision > 0, 'always_on_false_fixture');
  });
  await ctx.step('step_2', async () => {
    for (const value of [true, false]) {
      await ui.modal.getByLabel('Keep this Sandbox running when automatic pause is enabled', {exact: true}).setChecked(value);
      const updated = await poll(ctx, 'always_on_committed', () => read(ctx, f), r => r.body.sandbox.profile.always_on === value);
      latest = updated.body.sandbox;
      ctx.assert(latest.revision > original.revision, 'always_on_revision_did_not_advance');
    }
  });
  await ctx.step('step_3', async () => {
    await ctx.request({actor: 'admin', am: other(ctx.am), method: 'POST', path: path(f) + '/always-on',
      body: {always_on: true, expected_revision: original.revision}, allowed: [409]});
    await ctx.request({actor: 'read_operator', method: 'POST', path: path(f) + '/always-on',
      body: {always_on: true, expected_revision: latest.revision}, allowed: [403]});
    ctx.assert(equal((await read(ctx, f)).body.sandbox, latest), 'always_on_refusal_changed_state');
  });
  await ctx.step('expected_result', async () => {
    const audit = await f.audit({target: f.sandbox_id, action: 'update'});
    ctx.assert(audit.filter(r => r.outcome.status === 'succeeded').length === 2, 'always_on_audit_once_per_intent');
  });
}), define(36, ['admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet'); let before, home;
  await ctx.step('step_1', async () => {
    before = (await read(ctx, f)).body; home = await f.homeDigest();
    ctx.assert(before.runtime_update?.can_update === true && before.runtime_update.preserves_home === true, 'managed_update_fixture');
    ctx.assert(f.runtime_target.includes('@sha256:'), 'immutable_runtime_required');
  });
  await ctx.step('step_2', async () => {
    const queued = await ctx.request({actor: 'admin', method: 'POST', path: path(f) + '/runtime-update',
      body: {target_version: f.runtime_target}, allowed: [202]});
    ctx.assert(queued.body.runtime_update.target_version === f.runtime_target, 'runtime_target_identity');
    const result = await poll(ctx, 'runtime_update', () => read(ctx, f), r =>
      r.body.runtime_update?.current_version === f.runtime_target || r.body.runtime_update?.state === 'failed');
    ctx.assert(result.body.runtime_update.current_version === f.runtime_target && result.body.runtime_update.state !== 'failed', 'runtime_update_failed');
  });
  await ctx.step('step_3', async () => {
    ctx.assert(equal(await f.homeDigest(), home), 'runtime_home_changed');
    const active = await f.workWitness(); ctx.assert(active.running === true && active.interrupted === false, 'runtime_active_work_interrupted');
    const refused = await ctx.request({actor: 'admin', am: other(ctx.am), method: 'POST', path: path(f) + '/runtime-update',
      body: {target_version: f.runtime_unsupported_target}, allowed: [400, 409, 422]});
    ctx.assert(refused.status >= 400 && (await read(ctx, f)).body.runtime_update.current_version === f.runtime_target, 'unsupported_runtime_switched');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(equal(await f.homeDigest(), home), 'runtime_home_finality');
    const current = (await read(ctx, f)).body;
    ctx.assert(current.sandbox.id === f.sandbox_id && current.runtime_update.current_version === f.runtime_target, 'runtime_final_identity');
  });
}, 'global'), define(39, ['admin-fixtures'], async ctx => {
  const f = await take(ctx, 'fleet');
  await ctx.step('step_1', async () => {
    ctx.assert(['agent', 'workspace', 'sandbox'].every(kind => f.profiles.some(p => p.kind === kind && p.owner === f.other_owner)), 'cross_owner_profile_fixture');
    for (const p of f.profiles) await ctx.request({actor: 'user', method: 'POST', path: `/api/v1/admin/profiles/${p.kind}/${id(p.id)}/retire`, allowed: [403]});
  });
  await ctx.step('step_2', async () => {
    for (const p of f.profiles) {
      const retired = await ctx.request({actor: 'admin', method: 'POST', path: `/api/v1/admin/profiles/${p.kind}/${id(p.id)}/retire`});
      ctx.assert(retired.body.id === p.id && retired.body.kind === p.kind && !!retired.body.retired_at, 'profile_retirement_identity');
      const listed = await pages(ctx, '/api/v1/admin/profiles?' + new URLSearchParams({kind: p.kind, owner: p.owner}), {inspect: body => {
        const item = body.items.find(x => x.id === p.id); if (item) ctx.assert(!!item.retired_at, 'retired_profile_still_active');
      }});
      ctx.assert(Array.isArray(listed), 'profile_inventory_not_read');
      const use = await f.tryUseProfile(p); ctx.assert([400, 404, 409, 422].includes(use.status), 'retired_profile_admitted');
    }
  });
  await ctx.step('step_3', async () => {
    ctx.assert(f.protected_profiles.length >= 3, 'default_profiles_fixture');
    for (const p of f.protected_profiles) await ctx.request({actor: 'admin', method: 'POST', path: `/api/v1/admin/profiles/${p.kind}/${id(p.id)}/retire`, allowed: [409]});
  });
  await ctx.step('expected_result', async () => {
    for (const p of f.profiles) {
      const audit = await f.audit({target: p.id, action: 'delete'});
      ctx.assert(audit.filter(r => r.outcome.status === 'succeeded').length === 1, 'profile_retirement_audit');
      await ctx.request({actor: 'read_operator', method: 'PUT', path: `/api/v1/${p.kind}-profiles/${id(p.id)}`, body: p.update_body, key: `denied-${p.id}`, allowed: [403, 404, 409]});
    }
  });
}, 'global')];
