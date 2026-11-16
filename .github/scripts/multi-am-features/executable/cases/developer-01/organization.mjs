import {define, setup, required, roleRequest, browser, evidence, id, equal, other} from './common.mjs';
import {access, capabilities, interaction, revokedSend, agentRequest, digest} from './access-common.mjs';

export const orgInventory = define(90, 'agent', 2, async ctx => {
  const f = await setup(ctx, 'organization_inventory');
  required(f.organizationId && f.organizationLabel && f.agentId && f.sandboxId
    && typeof f.revokeMembership === 'function', 'admin_org_membership_fixture_missing');
  const sessions = [];
  await ctx.step('step_1', async () => {
    for (const [actor, role] of [['org_admin', 'admin'], ['org_member', 'member'], ['org_viewer', 'viewer']]) {
      const organizations = (await roleRequest(ctx, f, actor, 'GET', '/api/v1/organizations')).body;
      ctx.assert(organizations.items.some(o => o.id === f.organizationId && o.role === role),
        'admin_org_membership_not_current');
      const current = await access(ctx, f, actor);
      capabilities(ctx, current, ['view_details'], actor === 'org_admin' ? [] : ['manage_sharing', 'interact']);
      ctx.assert(current.owner?.organization_id === f.organizationId, 'admin_org_resource_scope');
    }
  });
  await ctx.step('step_2', async () => {
    for (const actor of ['org_admin', 'org_member', 'org_viewer']) {
      const s = await browser(ctx, actor, ctx.am, f.agentId); sessions.push(s);
      await s.page.goto(new URL('/', s.origin).href);
      await s.page.getByRole('button', {name: 'Navigation scope', exact: true}).click();
      await s.page.getByRole('menuitemradio', {name: f.organizationLabel + ' (Organization)', exact: true}).click();
      await s.page.getByText(f.agentName, {exact: true}).first().waitFor();
      await s.page.goto(new URL(f.sandboxUiPath, s.origin).href);
      await s.page.getByText(f.sandboxName, {exact: true}).first().waitFor();
      await s.page.goto(new URL(f.agentUiPath, s.origin).href);
      const label = actor === 'org_admin' ? 'Resource admin' : 'View only';
      await s.page.getByText(new RegExp('Your access: ' + label)).waitFor();
      ctx.assert(await s.page.getByText(new RegExp('Owner: ' + f.organizationLabel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))).isVisible(),
        'admin_org_ui_owner_attribution');
    }
  });
  await ctx.step('expected_result', async () => {
    await f.revokeMembership('org_admin');
    await agentRequest(ctx, f, 'org_admin', 'GET', '', undefined, {am: other(ctx.am), allowed: [403, 404]});
    await sessions[0].page.reload();
    await sessions[0].page.getByText(f.revokedNotice, {exact: false}).waitFor();
    ctx.assert(await sessions[0].page.getByText(/Your access: Resource admin/).count() === 0,
      'admin_revoked_org_ui_retained_authority');
    for (const s of sessions) await evidence(ctx, s);
  });
}, ['admin-fixtures', 'browser']);

export const orgInteraction = define(91, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'organization_interaction');
  required(f.filePath && f.content && f.targetSettings && f.narrow && f.wholeSandbox,
    'admin_org_interaction_scopes_missing');
  await ctx.step('step_1', async () => {
    await interaction(ctx, f, 'org_admin');
    const file = (await agentRequest(ctx, f, 'org_admin', 'POST', '/files',
      {path: f.filePath, content_base64: Buffer.from(f.content).toString('base64')})).body;
    ctx.assert(file.sha256 === digest(f.content), 'admin_org_file_write');
    const settings = (await agentRequest(ctx, f, 'org_admin', 'PATCH', '/settings', f.targetSettings,
      {key: f.keys.settings})).body;
    ctx.assert(['model', 'permission_mode', 'reasoning_effort'].every(k => settings[k] === f.targetSettings[k]),
      'admin_org_settings_write');
  });
  await ctx.step('step_2', async () => {
    for (const role of ['org_member', 'org_viewer', 'view_recipient']) {
      capabilities(ctx, await access(ctx, f, role), ['view_details'], ['interact', 'operate_agent']);
      await agentRequest(ctx, f, role, 'POST', '/input', f.inputFirst,
        {key: f.keys.denied + role, allowed: [403]});
      await agentRequest(ctx, f, role, 'PATCH', '/settings', f.targetSettings,
        {key: f.keys.denied + role + '-settings', allowed: [403]});
    }
    await revokedSend(ctx, f, 'org_admin', () => f.revokeMembership('org_admin'));
  });
  await ctx.step('step_3', async () => {
    const narrow = {...f, agentId: f.narrow.agentId};
    capabilities(ctx, await access(ctx, narrow, 'org_admin_narrow'), ['view_details'], ['interact']);
    await agentRequest(ctx, narrow, 'org_admin_narrow', 'POST', '/input', f.inputFirst,
      {key: f.keys.narrow, allowed: [403]});
    const whole = {...f, agentId: f.wholeSandbox.agentId};
    capabilities(ctx, await access(ctx, whole, 'org_admin_whole'), ['view_details', 'interact']);
    const response = await agentRequest(ctx, whole, 'org_admin_whole', 'POST', '/input', f.wholeSandbox.input,
      {key: f.keys.whole, allowed: [200, 202]});
    ctx.assert(response.status < 300, 'admin_whole_sandbox_grant_not_admitted');
  });
  await ctx.step('expected_result', async () => {
    const effects = await ctx.observe('admin_org_scope_effects', () => f.readScopeEffects(ctx));
    ctx.assert(effects.narrow === 0 && effects.whole_sandbox === 1 && effects.revoked === 0,
      'admin_org_scoped_effects_wrong');
  });
});

export const orgHistory = define(95, 'agent', 2, async ctx => {
  const f = await setup(ctx, 'organization_history');
  required(f.historyMarker && f.artifacts?.length && typeof f.holdHistoryRead === 'function'
    && typeof f.revokeMembership === 'function', 'admin_org_history_fixture_missing');
  await ctx.step('step_1', async () => {
    capabilities(ctx, await access(ctx, f, 'org_admin'), ['manage_bridge']);
    const history = (await agentRequest(ctx, f, 'org_admin', 'GET', '/channel-usage/history')).body;
    ctx.assert(JSON.stringify(history).includes(f.historyMarker), 'admin_org_channel_history_missing');
    for (const item of f.artifacts) {
      const response = await roleRequest(ctx, f, 'org_admin', 'GET', item.path, undefined, {agentId: f.agentId});
      ctx.assert(digest(typeof response.body === 'string' ? response.body : JSON.stringify(response.body)) === item.sha256,
        'admin_org_artifact_export_mismatch');
    }
    required(f.shareControl, 'admin_org_history_share_control_missing');
    const share = f.shareControl;
    const result = await roleRequest(ctx, f, 'org_admin', share.method, share.path, share.body,
      {key: share.key, agentId: f.agentId, allowed: share.allowed});
    ctx.assert(result.status === share.status && (share.status >= 400 || result.body.id),
      'admin_org_history_share_boundary');
  });
  await ctx.step('step_2', async () => {
    for (const role of ['control_operator', 'org_member']) {
      capabilities(ctx, await access(ctx, f, role), ['view_details'], ['manage_bridge']);
      await agentRequest(ctx, f, role, 'GET', '/channel-usage/history', undefined, {allowed: [403]});
    }
    const barrier = await f.holdHistoryRead(); ctx.defer('admin_history_barrier_cleanup', () => barrier.cleanup());
    const pending = agentRequest(ctx, f, 'org_admin', 'GET', '/channel-usage/history', undefined, {allowed: [403, 404]});
    // Attach rejection immediately so a barrier failure cannot leave an unhandled request promise.
    const settled = pending.then(value => ({value}), error => ({error}));
    await barrier.waitEntered(); await f.revokeMembership('org_admin'); await barrier.release();
    const result = await settled; if (result.error) throw result.error;
    ctx.assert(!JSON.stringify(result.value.body).includes(f.historyMarker), 'admin_revoked_inflight_history_disclosed');
  });
  await ctx.step('expected_result', async () => {
    await agentRequest(ctx, f, 'org_admin', 'GET', '/channel-usage/history', undefined,
      {am: other(ctx.am), allowed: [403, 404]});
    const final = await ctx.observe('admin_org_history_effect_counts', () => f.readEffects(ctx));
    ctx.assert(equal(final, f.expectedEffects), 'admin_org_history_unauthorized_effect');
  });
});
