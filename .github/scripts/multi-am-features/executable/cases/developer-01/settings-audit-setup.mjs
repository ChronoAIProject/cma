import {check, required, read, request, settingsSession, equal, P, other} from './common.mjs';
import {writableSettings} from './public-setups.mjs';
import {admissionAuditCohort} from './admission-audit.mjs';

export const matchesSettingsAudit = (row, revision) => row.action === 'configure'
  && row.outcome?.status === 'succeeded' && row.target?.kind === 'resource'
  && ['platform_settings', 'triggers'].includes(row.target.id)
  && row.detail?.revision === String(revision);

export async function settingsAuditSetup(ctx, {config}) {
  const actor = config.actors?.admin?.subject, limited = config.actors?.settings_operator?.subject;
  required(typeof actor === 'string' && actor.length > 0 && typeof limited === 'string'
    && limited.length > 0 && actor !== limited, 'admin_audit_exact_roles_required');
  // Refuse a missing/overprivileged role before changing settings. The browser
  // case repeats the check through its actual isolated cookie session.
  await request(ctx, 'GET', P + '/settings', undefined, {actor: 'settings_operator'});
  await request(ctx, 'GET', P + '/activity', undefined, {actor: 'settings_operator', allowed: [403]});
  const current = await writableSettings(ctx);
  required(current.settings.inactivity?.auto_pause_enabled === false
    && current.settings.triggers?.enabled === false && current.settings.model_budget?.enabled === false,
  'admin_audit_requires_disabled_background_policies');
  const cohort = await admissionAuditCohort(ctx, actor);
  // The provider acquired its real callback lease before invoking this factory.
  const state = await settingsSession(ctx, {exclusiveSettings: true});
  let cleanupJob;
  const cleanup = () => cleanupJob ??= (async () => {
    const errors = [];
    for (const operation of [cohort.cleanup, state.restore]) {
      try {await operation();} catch (error) {errors.push(error);}
    }
    if (errors.length) throw new AggregateError(errors, 'admin_audit_cleanup_failed');
  })();
  const days = state.original.settings.retention.agent_log_days === 30 ? 31 : 30;
  const changed = await state.patch('retention', {agent_log_days: days});
  check(equal(changed, await read(ctx, P + '/settings', other(ctx.am))), 'admin_audit_setup_peer_settings');
  const page = await read(ctx, P + '/activity?action=configure&limit=50');
  check(Array.isArray(page.items), 'admin_audit_setup_page_shape');
  const match = page.items.find(row => matchesSettingsAudit(row, changed.revision));
  check(match?.actor === actor && typeof match.id === 'string' && Number.isFinite(Date.parse(match.at)),
    'admin_audit_setup_receipt_missing');
  return {actor, cleanup, async useAuditCohort(name) {
    check(name === 'no_current_revision', 'admin_audit_unknown_cohort');
    const changes = await cohort.displace(match.at);
    const audit = await read(ctx, P + '/activity?action=configure&limit=50', other(ctx.am));
    check(Array.isArray(audit.items) && audit.items.length === 50
      && new Set(audit.items.map(row => row.id)).size === 50
      && new Set(audit.items.map(row => row.detail?.revision)).size === 50,
    'admin_audit_displacement_count');
    for (const row of audit.items) {
      const change = changes.find(item => String(item.revision) === row.detail?.revision);
      check(change && row.action === 'configure' && row.target?.kind === 'resource'
        && row.target.id === 'admission_control' && row.actor === actor
        && row.outcome?.status === 'succeeded' && Date.parse(row.at) > Date.parse(match.at)
        && Date.parse(row.at) === change.at * 1000
        && row.detail.status === (change.new_stop ? 'stopped' : 'open'),
      'admin_audit_displacement_receipt');
    }
    check(equal(changed, await read(ctx, P + '/settings')), 'admin_audit_cohort_changed_settings');
  }};
}
