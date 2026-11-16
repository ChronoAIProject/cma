import {check, required, read, request, equal, P} from './common.mjs';
import {failSettingsRead} from './settings-read-fault.mjs';

export async function settingsReadSetup(ctx, {config}) {
  const subjects = ['admin', 'settings_operator', 'read_operator'].map(role => config.actors?.[role]?.subject);
  required(subjects.every(subject => typeof subject === 'string' && subject.trim())
    && new Set(subjects).size === subjects.length, 'admin_read_exact_roles_required');
  const current = await read(ctx, P + '/settings'), budget = current.settings?.model_budget;
  check(Number.isSafeInteger(current.revision) && current.settings?.retention
    && (current.model_budget_writes_available === undefined
      || typeof current.model_budget_writes_available === 'boolean'), 'admin_read_settings_shape');
  required(current.model_budget_writes_available !== true, 'admin_read_requires_unavailable_writer');
  required(budget && typeof budget.enabled === 'boolean' && Array.isArray(budget.owners)
    && (budget.enabled || budget.platform != null || budget.default_policy != null
      || budget.owners.length > 0 || budget.request_models?.length > 0), 'admin_read_requires_retained_policy');
  const limited = await request(ctx, 'GET', P + '/settings', undefined, {actor: 'settings_operator'});
  check(limited.status === 200 && equal(limited.body, current), 'admin_read_settings_role_mismatch');
  const denied = await request(ctx, 'GET', P + '/settings', undefined,
    {actor: 'read_operator', allowed: [403]});
  check(denied.status === 403, 'admin_read_permission_fixture');
  // No mode switch, policy write or synthetic availability response. A parent
  // without this actual read-only mode remains BLOCKED before opening the UI.
  return {faults: {failNextRead: options => failSettingsRead(ctx, options)}, cleanup: async () => {}};
}
