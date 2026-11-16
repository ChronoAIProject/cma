import {randomUUID} from 'node:crypto';
import {requireResourceInputs, resources} from '../developer-17/setup-resources.mjs';
import {identity, prepareBrowser, requireBrowserInputs} from '../developer-17/setup-browser.mjs';
import {required, id, roleRequest} from './common.mjs';
import {agentRequest, access} from './access-common.mjs';
import {agentSettings} from './cross-controls-cleanup.mjs';

// The parent binds real principals and subordinate native custody. This recipe
// creates only a disposable public hierarchy; the case owns all control writes.
export function createCrossOwnerControlsFactory(bindings) {
  return async (ctx, {config}) => {
    requireResourceInputs(bindings);
    const roles = ['user', 'creator', 'control_user', 'control_operator', 'read_operator'];
    const subjects = roles.map(role => config.actors?.[role]?.subject);
    required(subjects.every(value => typeof value === 'string' && value.trim())
      && subjects[0] === subjects[1] && new Set(subjects.slice(1)).size === 4,
    'admin_cross_controls_exact_roles_required');
    required(identity(bindings, 'user').subject === subjects[0], 'admin_cross_controls_resource_owner');
    requireBrowserInputs(bindings, ['control_user', 'control_operator']);
    for (const role of ['control_user', 'control_operator']) {
      required(identity(bindings, role).subject === config.actors[role].subject,
        'admin_cross_controls_browser_identity');
    }
    const owned = await resources(ctx, bindings);
    const hierarchy = await owned.hierarchy();
    const f = {agentId: hierarchy.agentId, cleanup: owned.cleanup};
    for (const role of ['control_user', 'control_operator', 'read_operator']) {
      const current = await access(ctx, f, role);
      const allowed = ['view_details', 'view_history', 'view_files'];
      const denied = ['manage_sharing', 'manage_bridge'];
      if (role === 'read_operator') denied.push('operate_agent', 'interact', 'resolve_approval');
      else allowed.push('operate_agent', 'interact');
      required(current.owner?.kind === 'person' && current.owner.is_current_person === false
        && Array.isArray(current.capabilities) && allowed.every(c => current.capabilities.includes(c))
        && denied.every(c => !current.capabilities.includes(c)), 'admin_cross_controls_role_authority');
    }
    const current = (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body;
    required(current.pending === false && typeof current.model === 'string' && current.model.trim()
      && ['minimal', 'low', 'medium', 'high', 'x_high'].includes(current.reasoning_effort)
      && ['guarded', 'autonomous'].includes(current.permission_mode), 'admin_cross_controls_ready_settings');
    const preferences = (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body;
    required(preferences.prompt_supported === true && preferences.prompt_pending === false
      && typeof preferences.title === 'string' && preferences.title.trim()
      && typeof preferences.system_prompt === 'string' && Number.isSafeInteger(preferences.revision),
    'admin_cross_controls_ready_preferences');
    // Verify authenticated cookie-session capabilities, not a fixture claim or
    // the expected denial itself. A product violation belongs in the case result.
    for (const role of ['control_user', 'control_operator']) {
      const session = await prepareBrowser(ctx, bindings, {actor: role, agentId: f.agentId});
      required(session.capabilities.includes('control_any_sandbox')
        && session.capabilities.includes('use_own_sandboxes') === (role === 'control_user'),
      'admin_cross_controls_configuration_authority');
      await session.cleanup();
      await ctx.observe('admin_cross_controls_identity_evidence', () =>
        ctx.fixture('browser').evidence(session.session.page));
    }
    const head = (await roleRequest(ctx, f, 'creator', 'GET',
      '/api/v2/agents/' + id(f.agentId) + '/events/head', undefined, {agentId: f.agentId})).body;
    required(head.agent_id === f.agentId && head.object === 'cma.agent.event_head'
      && head.active_turn === null, 'admin_cross_controls_idle_required');
    const filePath = 'admin01-controls-' + randomUUID() + '.txt';
    const tree = (await agentRequest(ctx, f, 'creator', 'GET', '/files/tree')).body;
    required(Array.isArray(tree.entries) && !tree.entries.some(entry => entry.path === filePath),
      'admin_cross_controls_unique_file_required');
    return {...f, targetSettings: {...agentSettings(current),
      permission_mode: current.permission_mode === 'guarded' ? 'autonomous' : 'guarded'},
    title: 'Local control ' + randomUUID(), prompt: 'Local disposable control fixture ' + randomUUID(),
    filePath, content: 'Local control file ' + randomUUID() + '\n',
    attachmentName: 'admin01-attachment-' + randomUUID() + '.txt',
    keys: Object.fromEntries(['settings', 'restoreSettings', 'title', 'system_prompt', 'stop']
      .map(name => [name, randomUUID()]))};
  };
}
