// Adapter for the sole parent's existing local topology and settings lease.
// It starts no services. Named factories must use public setup APIs/fault seams;
// unavailable factories stay BLOCKED, never synthetic successful responses.
import {validateTopology} from '../../participant.mjs';
import {Blocked} from '../../runner.mjs';
import {read, P, check} from './common.mjs';

const globalSetups = new Set([
  'settings_retention', 'settings_read', 'settings_conflict', 'settings_patch', 'settings_audit',
  'idle_preview', 'idle_pause', 'automatic_pause', 'user_limit', 'visitor_limits',
  'measured_visibility', 'defaults_marketplace', 'trigger_settings', 'trigger_commands',
  'admission_read', 'admission_change', 'spend_read', 'spend_observation',
  'spend_availability', 'spend_period', 'default_owner_policy', 'owner_overrides', 'request_models',
]);
const safeError = error => /^[a-z][a-z0-9_]{0,100}$/.test(error?.message ?? '')
  ? error.message : 'admin_setup_operation_failed';

// The parent's callback lease remains held across all case steps and cleanup.
// Rejection/timeout cancels a queued callback before it can acquire and mutate.
export async function holdParentLease(withExclusiveSettings, timeoutMs = 30000) {
  if (typeof withExclusiveSettings !== 'function') throw new Blocked('admin_parent_settings_lease_missing');
  let enteredResolve, enteredReject, releaseResolve, entered = false, cancelled = false;
  const enteredPromise = new Promise((resolve, reject) => {enteredResolve = resolve; enteredReject = reject;});
  const released = new Promise(resolve => {releaseResolve = resolve;});
  const job = Promise.resolve().then(() => withExclusiveSettings(async () => {
    if (cancelled) return;
    check(!entered, 'admin_parent_lease_callback_replayed'); entered = true; enteredResolve();
    await released;
  }));
  // Never leave rejection of a late parent callback unobserved.
  const settled = job.then(() => {
    if (!entered && !cancelled) enteredReject(Error('admin_parent_lease_callback_missing'));
    return null;
  }, error => {enteredReject(error); return error;});
  let timer;
  try {
    await Promise.race([enteredPromise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Blocked('admin_parent_lease_timeout')), timeoutMs);
    })]);
  } catch (error) {cancelled = true; releaseResolve(); throw error;}
  finally {clearTimeout(timer);}
  let releasedOnce = false;
  return async () => {
    if (!releasedOnce) {releasedOnce = true; releaseResolve();}
    const failure = await settled; if (failure) throw failure;
  };
}

function validateLocalConfig(config) {
  check(config.scope === 'LOCAL_DISPOSABLE_TWO_AM' && config.database_disposable === true
    && config.restart_authority === 'PARENT_ONLY', 'admin_parent_scope_required');
  for (const origin of Object.values(config.am_origins ?? {})) {
    const url = new URL(origin);
    check(['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
      && ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password,
    'admin_parent_nonlocal_origin');
  }
  check(config.am_origins?.a && config.am_origins?.b && config.am_origins.a !== config.am_origins.b,
    'admin_parent_two_origins');
  check(/^[a-f0-9]{40}$/.test(config.expected_api_source ?? '')
    && /@sha256:[a-f0-9]{64}$/.test(config.expected_api_image ?? ''), 'admin_parent_image_binding');
}

export function createAdminFixtures({config, adapter, withExclusiveSettings, factories = {}}) {
  validateLocalConfig(config);
  check(typeof adapter?.topology === 'function', 'admin_parent_topology_reader');
  const active = new Set();
  // Retention is a concrete setup requiring no native resource fabrication:
  // current public settings plus the parent's held global lease are sufficient.
  const implementations = {settings_retention: async ctx => {
    const current = await read(ctx, P + '/settings');
    check(Number.isSafeInteger(current.revision) && current.settings.retention, 'admin_settings_fixture_read');
    if (current.settings.model_budget?.enabled && current.model_budget_writes_available !== true) {
      throw new Blocked('admin_retention_spend_gate_closed');
    }
    return {cleanup: async () => {}};
  }, ...factories};
  return {
    supportedSetups: Object.keys(implementations).sort(),
    capabilities: Object.fromEntries(Object.entries(implementations)
      .map(([name, factory]) => ['admin-setup-' + name.replaceAll('_', '-'), factory])),
    async take(name, ctx) {
      const factory = implementations[name];
      if (typeof factory !== 'function') throw new Blocked('admin_setup_unavailable_' + name);
      const key = ctx.row.case_id + ':' + ctx.phase + ':' + ctx.iteration;
      check(!active.has(key), 'admin_setup_replayed'); active.add(key);
      let release, fixture, cleaned = false;
      const cleanup = async () => {
        if (cleaned) return; cleaned = true;
        const errors = [];
        try { await fixture?.cleanup(); } catch (error) {errors.push(error);}
        try { await release?.(); } catch (error) {errors.push(error);}
        active.delete(key);
        if (errors.length) throw new AggregateError(errors, 'admin_fixture_cleanup_failed');
      };
      // Register before acquisition/setup; failed partial setup still releases custody.
      ctx.defer('admin_parent_fixture_release', cleanup);
      try {
        if (globalSetups.has(name)) release = await holdParentLease(withExclusiveSettings);
        const topology = validateTopology(await adapter.topology(), config.expected_api_image);
        await ctx.observe('admin_live_fixture_topology', async () => ({source: config.expected_api_source,
          image: config.expected_api_image, topology}));
        fixture = await factory(ctx);
        check(typeof fixture?.cleanup === 'function', 'admin_setup_cleanup_contract');
        return {...fixture, scope: config.scope, createdByRun: true,
          exclusiveSettings: !!release, exclusiveAdmission: !!release, cleanup};
      } catch (primary) {
        try {await cleanup();} catch (cleanupError) {
          // Keep original failure plus the cleanup failure for parent evidence.
          await ctx.observe('admin_failed_setup_cleanup', async () => ({primary: safeError(primary),
            cleanup: safeError(cleanupError)}));
          throw new AggregateError([primary, cleanupError], 'admin_setup_and_cleanup_failed');
        }
        throw primary;
      }
    },
  };
}
