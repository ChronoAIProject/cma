import {define, setup, required, browser, evidence, other, equal, id} from './common.mjs';
import {agentRequest, access, capabilities, digest} from './access-common.mjs';
import {poll} from './enforcement.mjs';
import {agentSettings as settings, restoreAgentSettings, restoreAgentPreference,
  removeControlFile} from './cross-controls-cleanup.mjs';

export const crossOwnerControls = define(82, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'cross_owner_controls');
  required(f.targetSettings && f.title && f.prompt && f.filePath && f.content && f.attachmentName,
    'admin_cross_owner_controls_fixture_missing');
  let initialSettings, originalPreferences, attachment, s;
  let restoreSettings, removeFile, settingsAM, fileAM, preferencesAM;
  const restorePreferences = [];
  const query = '?' + new URLSearchParams({path: f.filePath});
  await ctx.step('step_1', async () => {
    capabilities(ctx, await access(ctx, f, 'control_user'), ['operate_agent', 'view_files']);
    initialSettings = (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body;
    required(initialSettings.pending === false && !equal(settings(initialSettings), f.targetSettings),
      'admin_changed_ready_settings_required');
    restoreSettings = restoreAgentSettings(ctx, f, initialSettings);
    const ack = await agentRequest(ctx, f, 'control_user', 'PATCH', '/settings', f.targetSettings,
      {key: f.keys.settings});
    settingsAM = ack.timing.am;
    ctx.assert(['a', 'b'].includes(settingsAM) && equal(settings(ack.body), f.targetSettings),
      'admin_cross_owner_settings_ack');
    await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body,
      value => value.pending === false && equal(settings(value), f.targetSettings), 'admin_settings_effect_timeout');
    await agentRequest(ctx, f, 'control_operator', 'GET', '/files/content' + query, undefined, {allowed: [404]});
    removeFile = removeControlFile(ctx, f);
    const file = await agentRequest(ctx, f, 'control_operator', 'POST', '/files',
      {path: f.filePath, content_base64: Buffer.from(f.content).toString('base64')});
    fileAM = file.timing.am;
    ctx.assert(['a', 'b'].includes(fileAM) && file.body.path === f.filePath
      && file.body.sha256 === digest(f.content), 'admin_cross_owner_file_write');
    s = await browser(ctx, 'control_user', ctx.am, f.agentId);
    await s.page.goto(new URL('/agents/' + id(f.agentId), s.origin).href);
    const upload = s.page.waitForResponse(r => new URL(r.url()).pathname.endsWith('/attachments')
      && r.request().method() === 'POST').then(response => ({response}), error => ({error}));
    await s.page.locator('input[type="file"]').setInputFiles({name: f.attachmentName,
      mimeType: 'text/plain', buffer: Buffer.from(f.content)});
    const outcome = await upload; if (outcome.error) throw outcome.error;
    const response = outcome.response; attachment = await response.json();
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
      restorePreferences.push(restoreAgentPreference(ctx, f, field, originalPreferences[field], target));
      const ack = await agentRequest(ctx, f, 'control_user', 'PATCH', '/preferences',
        {expected_revision: before.revision, [field]: target}, {key: f.keys[field]});
      const changed = ack.body; preferencesAM = ack.timing.am;
      ctx.assert(['a', 'b'].includes(preferencesAM) && changed[field] === target
        && changed.revision > before.revision, 'admin_preference_changed');
      await agentRequest(ctx, f, 'control_operator', 'PATCH', '/preferences',
        {expected_revision: changed.revision, [field]: originalPreferences[field]}, {key: f.keys[field] + '-denied', allowed: [403]});
    }
    await agentRequest(ctx, f, 'control_operator', 'GET', '/preferences', undefined, {allowed: [403]});
    await agentRequest(ctx, f, 'control_operator', 'PATCH', '/settings', f.targetSettings,
      {key: f.keys.settings + '-denied', allowed: [403]});
    await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body,
      value => value.prompt_pending === false, 'admin_prompt_settlement_timeout');
    await s.page.reload();
    await s.page.getByRole('button', {name: 'More Agent actions', exact: true}).click();
    await s.page.getByRole('menu', {name: 'More Agent actions', exact: true}).waitFor();
    const sharedRename = s.page.getByRole('menuitem', {name: 'Rename', exact: true});
    // AgentHeader renders unavailable owner actions as disabled menu items.
    ctx.assert(await sharedRename.count() === 0 || await sharedRename.isDisabled(),
      'admin_shared_ui_owner_rename_exposed');
    await s.page.keyboard.press('Escape');
    const ownerSession = await browser(ctx, 'creator', other(ctx.am), f.agentId);
    await ownerSession.page.goto(new URL('/agents/' + id(f.agentId), ownerSession.origin).href);
    await ownerSession.page.getByRole('button', {name: 'More Agent actions', exact: true}).click();
    await ownerSession.page.getByRole('menu', {name: 'More Agent actions', exact: true}).waitFor();
    const ownerRename = ownerSession.page.getByRole('menuitem', {name: 'Rename', exact: true});
    ctx.assert(await ownerRename.isVisible() && !(await ownerRename.isDisabled()),
      'admin_owner_rename_control_missing');
    await ownerSession.page.keyboard.press('Escape'); await evidence(ctx, ownerSession);
  });
  await ctx.step('step_3', async () => {
    const peerSettings = (await agentRequest(ctx, f, 'control_user', 'GET', '/settings', undefined,
      {am: other(settingsAM)})).body;
    ctx.assert(peerSettings.pending === false && equal(settings(peerSettings), f.targetSettings),
      'admin_settings_peer_mismatch');
    const preferences = await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET',
      '/preferences', undefined, {am: other(preferencesAM)})).body, value => value.prompt_pending === false,
    'admin_prompt_settlement_timeout');
    ctx.assert(preferences.title === f.title && preferences.system_prompt === f.prompt,
      'admin_preferences_peer_mismatch');
    const content = (await agentRequest(ctx, f, 'read_operator', 'GET', '/files/content' + query,
      undefined, {am: other(fileAM)})).body;
    ctx.assert(content.path === f.filePath && content.sha256 === digest(f.content)
      && Buffer.from(content.content_base64, 'base64').toString() === f.content, 'admin_file_peer_mismatch');
    await removeFile();
    // Restore mutable settings while the runtime still serves; Stop then tests its distinct lifecycle.
    await restoreSettings();
    for (const restore of restorePreferences) await restore();
    await agentRequest(ctx, f, 'control_operator', 'POST', '/stop', undefined, {key: f.keys.stop, allowed: [200, 202]});
    const stopped = await poll(async () => (await agentRequest(ctx, f, 'creator', 'GET')).body,
      value => value.status === 'stopped', 'admin_stop_not_complete');
    ctx.assert(!['retired', 'retiring'].includes(stopped.sandbox.status), 'admin_stop_retired_sandbox');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, s); });
}, ['admin-fixtures', 'browser']);
