// Adapter for the sole parent's existing local topology and settings lease.
// It starts no services. Named factories must use public setup APIs/fault seams;
// unavailable factories stay BLOCKED, never synthetic successful responses.
import {validateTopology} from '../../participant.mjs';
import {Blocked} from '../../runner.mjs';
import {check} from './common.mjs';
import {publicSetups} from './public-setups.mjs';
import {settingsPatchSetup} from './settings-setup.mjs';

const globalSetups = new Set([
  'settings_retention', 'settings_read', 'settings_conflict', 'settings_patch', 'settings_audit',
  'idle_preview', 'idle_pause', 'automatic_pause', 'user_limit', 'visitor_limits',
  'measured_visibility', 'defaults_marketplace', 'trigger_settings', 'trigger_commands',
  'admission_read', 'admission_change', 'spend_read', 'spend_observation',
  'spend_availability', 'spend_period', 'default_owner_policy', 'owner_overrides', 'request_models',
]);
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
  const origins = config.am_origins ?? config.ams;
  for (const origin of Object.values(origins ?? {})) {
    const url = new URL(origin);
    check(['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
      && ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password
      && url.pathname === '/' && !url.search && !url.hash,
    'admin_parent_nonlocal_origin');
  }
  check(origins?.a && origins?.b && new URL(origins.a).origin !== new URL(origins.b).origin,
    'admin_parent_two_origins');
  check(/^[a-f0-9]{40}$/.test(config.expected_api_source ?? '')
    && /@sha256:[a-f0-9]{64}$/.test(config.expected_api_image ?? ''), 'admin_parent_image_binding');
}

export function createAdminFixtures({config, adapter, withExclusiveSettings, factories = {}}) {
  validateLocalConfig(config);
  check(typeof adapter?.topology === 'function', 'admin_parent_topology_reader');
  const active = new Set();
  const implementations = {...publicSetups, settings_patch: settingsPatchSetup, ...factories};
  check(Object.entries(implementations).every(([name, factory]) => /^[a-z][a-z0-9_]*$/.test(name)
    && typeof factory === 'function'), 'admin_factory_contract');
  const provider = {
    supportedSetups: Object.keys(implementations).sort(),
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
      if (globalSetups.has(name)) release = await holdParentLease(withExclusiveSettings);
      const topology = validateTopology(await adapter.topology(), config.expected_api_image);
      await ctx.observe('admin_live_fixture_topology', async () => ({source: config.expected_api_source,
        image: config.expected_api_image, topology,
        final_database_cleanup: 'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS'}));
      // On setup failure, the runner unwinds later resource defers before this
      // earlier lease defer. Releasing here would let the next case change
      // global policy while partially created resources still need cleanup.
      fixture = await factory(ctx, {config});
      check(typeof fixture?.cleanup === 'function', 'admin_setup_cleanup_contract');
      return {...fixture, scope: config.scope, createdByRun: true,
        exclusiveSettings: !!release, exclusiveAdmission: !!release, cleanup};
    },
  };
  // Author-owned setup dispatch must pass through the same lease, exact-image
  // observation and cleanup wrapper as the legacy parent-installed adapter.
  provider.capabilities = Object.fromEntries(Object.keys(implementations)
    .map(name => ['admin-setup-' + name.replaceAll('_', '-'), ctx => provider.take(name, ctx)]));
  return provider;
}
