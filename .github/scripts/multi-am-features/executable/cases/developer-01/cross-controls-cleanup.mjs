import {check, equal} from './common.mjs';
import {agentRequest, digest} from './access-common.mjs';
import {poll} from './enforcement.mjs';

export const agentSettings = body => ({model: body.model, reasoning_effort: body.reasoning_effort,
  permission_mode: body.permission_mode});

// Explicit restoration and reverse unwinding share the same attempt, including
// rejection. A later read cannot turn an unknown write into permission to replay.
function deferredOnce(ctx, label, operation) {
  let job;
  const run = () => job ??= Promise.resolve().then(operation);
  ctx.defer(label, run);
  return run;
}

export function restoreAgentSettings(ctx, f, initial) {
  return deferredOnce(ctx, 'admin_agent_settings_restore', async () => {
    const current = (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body;
    if (!equal(agentSettings(current), agentSettings(initial))) {
      check(equal(agentSettings(current), f.targetSettings), 'admin_agent_restore_foreign_settings');
      await agentRequest(ctx, f, 'control_user', 'PATCH', '/settings', agentSettings(initial),
        {key: f.keys.restoreSettings});
    }
    await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/settings')).body,
      value => !value.pending && equal(agentSettings(value), agentSettings(initial)), 'admin_settings_restore_timeout');
  });
}

export function restoreAgentPreference(ctx, f, field, original, target) {
  return deferredOnce(ctx, 'admin_' + field + '_restore', async () => {
    const current = (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body;
    if (current[field] !== original) {
      check(current[field] === target, 'admin_preference_cleanup_foreign_value');
      await agentRequest(ctx, f, 'control_user', 'PATCH', '/preferences',
        {expected_revision: current.revision, [field]: original}, {key: f.keys[field] + '-restore'});
    }
    await poll(async () => (await agentRequest(ctx, f, 'control_user', 'GET', '/preferences')).body,
      value => !value.prompt_pending && value[field] === original, 'admin_preferences_restore_timeout');
  });
}

export function removeControlFile(ctx, f) {
  const query = '?' + new URLSearchParams({path: f.filePath});
  return deferredOnce(ctx, 'admin_fixture_file_remove', async () => {
    const response = await agentRequest(ctx, f, 'creator', 'GET', '/files/content' + query,
      undefined, {allowed: [200, 404]});
    if (response.status === 200) {
      check(response.body.path === f.filePath && response.body.sha256 === digest(f.content),
        'admin_file_cleanup_foreign_bytes');
      await agentRequest(ctx, f, 'creator', 'DELETE', '/files' + query);
    }
    await agentRequest(ctx, f, 'creator', 'GET', '/files/content' + query, undefined, {allowed: [404]});
  });
}
