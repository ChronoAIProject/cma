import {define, setup, required, browser, evidence, other, equal, id, check} from './common.mjs';
import {agentRequest, access, capabilities, digest} from './access-common.mjs';
import {poll} from './enforcement.mjs';

const settings = body => ({model: body.model, reasoning_effort: body.reasoning_effort,
  permission_mode: body.permission_mode});

export const crossOwnerControls = define(82, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'cross_owner_controls');
  required(f.targetSettings && f.title && f.prompt && f.filePath && f.content && f.attachmentName,
    'admin_cross_owner_controls_fixture_missing');
  let initialSettings, originalPreferences, attachment, s;
  let settingsRestored = false, fileRemoved = false, preferencesRestored = false;
  const query = '?' + new URLSearchParams({path: f.filePath});
  await ctx.step('step_1', async () => {
    capabilities(ctx, await access(ctx, f, 'control_user'), ['operate_agent', 'view_files']);
    initialSettings = (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body;
    required(initialSettings.pending === false && !equal(settings(initialSettings), f.targetSettings),
      'admin_changed_ready_settings_required');
    ctx.defer('admin_agent_settings_restore', async () => {
      if (settingsRestored) return;
      const current = (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body;
      if (!equal(settings(current), settings(initialSettings))) {
        check(equal(settings(current), f.targetSettings), 'admin_agent_restore_foreign_settings');
        await agentRequest(ctx, f, 'control_user', 'PATCH', '/settings', settings(initialSettings),
          {key: f.keys.restoreSettings});
      }
      await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body,
        value => !value.pending && equal(settings(value), settings(initialSettings)), 'admin_settings_restore_timeout');
      settingsRestored = true;
    });
    const ack = (await agentRequest(ctx, f, 'control_user', 'PATCH', '/settings', f.targetSettings,
      {key: f.keys.settings})).body;
    ctx.assert(equal(settings(ack), f.targetSettings), 'admin_cross_owner_settings_ack');
    await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body,
      value => value.pending === false && equal(settings(value), f.targetSettings), 'admin_settings_effect_timeout');
    await agentRequest(ctx, f, 'control_operator', 'GET', '/files/content' + query, undefined, {allowed: [404]});
    ctx.defer('admin_fixture_file_remove', async () => {
      if (fileRemoved) return;
      const response = await agentRequest(ctx, f, 'creator', 'GET', '/files/content' + query, undefined, {allowed: [200, 404]});
      if (response.status === 200) {
        check(response.body.sha256 === digest(f.content), 'admin_file_cleanup_foreign_bytes');
        await agentRequest(ctx, f, 'creator', 'DELETE', '/files' + query);
      }
      fileRemoved = true;
    });
    const file = (await agentRequest(ctx, f, 'control_operator', 'POST', '/files',
      {path: f.filePath, content_base64: Buffer.from(f.content).toString('base64')})).body;
    ctx.assert(file.sha256 === digest(f.content), 'admin_cross_owner_file_write');
    s = await browser(ctx, 'control_user', ctx.am, f.agentId);
    await s.page.goto(new URL('/agents/' + id(f.agentId), s.origin).href);
    const upload = s.page.waitForResponse(r => new URL(r.url()).pathname.endsWith('/attachments')
      && r.request().method() === 'POST');
    await s.page.locator('input[type="file"]').setInputFiles({name: f.attachmentName,
      mimeType: 'text/plain', buffer: Buffer.from(f.content)});
    const response = await upload; attachment = await response.json();
    ctx.assert(response.status() === 201 && attachment.agent_id === f.agentId
      && attachment.sha256 === digest(f.content), 'admin_cross_owner_attachment');
    await ctx.observe('admin_attachment_receipt', async () => attachment);
  });
  await ctx.step('step_2', async () => {
    originalPreferences = (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body;
    required(originalPreferences.prompt_supported && !originalPreferences.prompt_pending
      && originalPreferences.title && originalPreferences.title !== f.title
      && originalPreferences.system_prompt !== f.prompt, 'admin_changed_preferences_fixture_required');
    for (const [field, target] of [['title', f.title], ['system_prompt', f.prompt]]) {
      const before = (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body;
      ctx.defer('admin_' + field + '_restore', async () => {
        if (preferencesRestored) return;
        const current = (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body;
        if (current[field] !== originalPreferences[field]) {
          check(current[field] === target, 'admin_preference_cleanup_foreign_value');
          await agentRequest(ctx, f, 'control_user', 'PATCH', '/preferences',
            {expected_revision: current.revision, [field]: originalPreferences[field]}, {key: f.keys[field] + '-restore'});
        }
      });
      const changed = (await agentRequest(ctx, f, 'control_user', 'PATCH', '/preferences',
        {expected_revision: before.revision, [field]: target}, {key: f.keys[field]})).body;
      ctx.assert(changed[field] === target && changed.revision > before.revision, 'admin_preference_changed');
      await agentRequest(ctx, f, 'control_operator', 'PATCH', '/preferences',
        {expected_revision: changed.revision, [field]: originalPreferences[field]}, {key: f.keys[field] + '-denied', allowed: [403]});
    }
    await agentRequest(ctx, f, 'control_operator', 'GET', '/preferences', undefined, {allowed: [403]});
    await agentRequest(ctx, f, 'control_operator', 'PATCH', '/settings', f.targetSettings,
      {key: f.keys.settings + '-denied', allowed: [403]});
    await s.page.reload();
    await s.page.getByRole('button', {name: 'More Agent actions', exact: true}).click();
    ctx.assert(await s.page.getByRole('menuitem', {name: 'Rename', exact: true}).count() === 0,
      'admin_shared_ui_owner_rename_exposed');
    await s.page.keyboard.press('Escape');
    const ownerSession = await browser(ctx, 'creator', other(ctx.am), f.agentId);
    await ownerSession.page.goto(new URL('/agents/' + id(f.agentId), ownerSession.origin).href);
    await ownerSession.page.getByRole('button', {name: 'More Agent actions', exact: true}).click();
    ctx.assert(await ownerSession.page.getByRole('menuitem', {name: 'Rename', exact: true}).isVisible(),
      'admin_owner_rename_control_missing');
    await ownerSession.page.keyboard.press('Escape'); await evidence(ctx, ownerSession);
  });
  await ctx.step('step_3', async () => {
    const preferences = await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET',
      '/preferences', undefined, {am: other(ctx.am)})).body, value => value.prompt_pending === false,
    'admin_prompt_settlement_timeout');
    ctx.assert(preferences.title === f.title && preferences.system_prompt === f.prompt,
      'admin_preferences_peer_mismatch');
    const content = (await agentRequest(ctx, f, 'read_operator', 'GET', '/files/content' + query,
      undefined, {am: other(ctx.am)})).body;
    ctx.assert(content.sha256 === digest(f.content), 'admin_file_peer_mismatch');
    await agentRequest(ctx, f, 'creator', 'DELETE', '/files' + query);
    await agentRequest(ctx, f, 'creator', 'GET', '/files/content' + query, undefined, {allowed: [404]});
    fileRemoved = true;
    // Restore mutable settings while the runtime still serves; Stop then tests its distinct lifecycle.
    await agentRequest(ctx, f, 'control_user', 'PATCH', '/settings', settings(initialSettings), {key: f.keys.restoreSettings});
    await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body,
      value => !value.pending && equal(settings(value), settings(initialSettings)), 'admin_settings_restore_timeout');
    settingsRestored = true;
    for (const field of ['title', 'system_prompt']) {
      const current = (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body;
      await agentRequest(ctx, f, 'control_user', 'PATCH', '/preferences',
        {expected_revision: current.revision, [field]: originalPreferences[field]}, {key: f.keys[field] + '-restore'});
    }
    await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body,
      value => !value.prompt_pending && value.title === originalPreferences.title
        && value.system_prompt === originalPreferences.system_prompt, 'admin_preferences_restore_timeout');
    preferencesRestored = true;
    await agentRequest(ctx, f, 'control_operator', 'POST', '/stop', undefined, {key: f.keys.stop, allowed: [200, 202]});
    const stopped = await poll(async () => (await agentRequest(ctx, f, 'creator', 'GET')).body,
      value => value.status === 'stopped', 'admin_stop_not_complete');
    ctx.assert(!['retired', 'retiring'].includes(stopped.sandbox.status), 'admin_stop_retired_sandbox');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, s); });
}, ['admin-fixtures', 'browser']);
