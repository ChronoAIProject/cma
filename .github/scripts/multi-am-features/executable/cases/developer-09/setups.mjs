import {check, requireSetup, request, wait} from './setup-common.mjs';
import {prepareUsage, basicUsageCases} from './setup-usage.mjs';

// The parent owns the local auth issuer. These recipes only ask it to read and
// CAS-replace named grants; they never discover credentials or manufacture an
// AM response. A role's exact capabilities are part of the setup contract so
// the browser and API cases exercise the same authorization boundary.
export const roleCapabilities = Object.freeze({
  read_operator: ['read_all_sandboxes'],
  control_operator: ['control_any_sandbox'],
  settings_operator: ['configure_platform'],
  activity_operator: ['read_activity_log'],
  directory_operator: ['read_all_sandboxes', 'read_activity_log', 'read_user_directory'],
  names_operator: ['read_all_sandboxes', 'read_activity_log'],
  runtime_operator: ['read_runtime_inventory'],
});

const roleNames = Object.freeze(Object.keys(roleCapabilities));
const sorted = value => [...value].sort();
const sameCapabilities = (left, right) => JSON.stringify(sorted(left)) === JSON.stringify(sorted(right));

function requireAuth(primitives) {
  const auth = primitives?.auth;
  requireSetup(typeof auth?.readGrant === 'function' && typeof auth?.replaceGrant === 'function',
    'admin09_auth_grant_primitives_required');
  return auth;
}

function requireIdentity(identities, actor) {
  const identity = identities?.[actor];
  requireSetup(identity?.actor === actor && typeof identity.subject === 'string' && identity.subject.length > 0,
    'admin09_named_identity_required_' + actor);
  return identity;
}

function grantShape(grant, actor, identity) {
  check(grant?.actor === actor && grant.subject === identity.subject
    && Number.isSafeInteger(grant.revision) && grant.revision > 0
    && Array.isArray(grant.capabilities) && grant.capabilities.every(value => typeof value === 'string'),
  'admin09_grant_shape');
  return {actor, subject: grant.subject, revision: grant.revision, capabilities: [...grant.capabilities]};
}

async function readGrant(auth, identities, actor) {
  const identity = requireIdentity(identities, actor);
  return grantShape(await auth.readGrant({actor}), actor, identity);
}

async function replaceGrant(auth, identities, actor, expectedRevision, capabilities) {
  const identity = requireIdentity(identities, actor);
  const result = await auth.replaceGrant({actor, expected_revision: expectedRevision, capabilities: [...capabilities]});
  const value = grantShape(result, actor, identity);
  check(value.revision !== expectedRevision && sameCapabilities(value.capabilities, capabilities), 'admin09_grant_replace_receipt');
  return value;
}

export async function prepareAdminRoles(ctx, primitives) {
  const auth = requireAuth(primitives), identities = primitives.identities;
  for (const actor of ['admin', 'user', 'outsider', ...roleNames]) requireIdentity(identities, actor);

  const original = new Map();
  for (const actor of roleNames) original.set(actor, await readGrant(auth, identities, actor));
  let restored = false;
  const restore = async () => {
    if (restored) return;
    const failures = [];
    for (const [actor, initial] of original) {
      try {
        const current = await readGrant(auth, identities, actor);
        if (!sameCapabilities(current.capabilities, initial.capabilities))
          await replaceGrant(auth, identities, actor, current.revision, initial.capabilities);
      } catch (error) { failures.push(error); }
    }
    if (failures.length) throw new AggregateError(failures, 'admin09_auth_restore_failed');
    restored = true;
  };
  // This is registered before the first grant write, including partial setup
  // failures. The returned cleanup below is idempotent for the parent dispatcher.
  ctx.defer('admin09_auth_restore_partial', restore);

  for (const actor of roleNames) {
    const current = await readGrant(auth, identities, actor);
    const wanted = roleCapabilities[actor];
    if (!sameCapabilities(current.capabilities, wanted)) await replaceGrant(auth, identities, actor, current.revision, wanted);
  }

  // Read both ingress sessions after the grant writes. This binds the role
  // recipe to the actual auth/session response, rather than trusting the grant
  // service response as a proxy for AM authorization.
  const otherAm = ctx.am === 'a' ? 'b' : 'a';
  for (const actor of roleNames) {
    for (const am of [ctx.am, otherAm]) {
      const session = await request(ctx, {actor, am, method: 'GET', path: '/auth/session', allowed: [200]});
      check(session?.authenticated === true && session.subject === identities[actor].subject
        && sameCapabilities(session.capabilities ?? [], roleCapabilities[actor]), 'admin09_auth_session_mismatch');
    }
  }

  let usage;
  if (ctx.row.case_id === 'C275:ADM-003') {
    usage = await prepareUsage(ctx, primitives, true);
    const made = await request(ctx, {actor: 'admin', method: 'POST', path: '/api/v1/admin/usage/metrics/reports',
      body: usage.report_query, allowed: [200, 202]});
    check(typeof made?.report_id === 'string' && made.report_id.length > 0, 'admin09_role_report_identity');
    const report = await wait(() => request(ctx, {actor: 'admin', method: 'GET',
      path: '/api/v1/admin/usage/metrics/reports/' + encodeURIComponent(made.report_id)}),
      value => value?.status === 'ready' || value?.status === 'failed', 'admin09_role_report_timeout');
    check(report.status === 'ready', 'admin09_role_report_failed');
    usage.report_id = made.report_id;
  }

  const cleanup = async () => { await usage?.cleanup?.(); await restore(); };
  return {
    scope: 'LOCAL_DISPOSABLE_TWO_AM', cleanup,
    sandbox_id: usage?.sandbox_ids?.[0] ?? null,
    report_id: usage?.report_id ?? null,
    private_markers: usage?.private_markers ?? [],
    revoke: async (actor, capability) => revokeGrant(auth, identities, actor, capability),
    revokeDirectory: async actor => revokeGrant(auth, identities, actor, 'read_user_directory'),
  };
}

async function revokeGrant(auth, identities, actor, capability) {
  const current = await readGrant(auth, identities, actor);
  check(current.capabilities.includes(capability), 'admin09_revoke_capability_missing');
  const next = current.capabilities.filter(value => value !== capability);
  await replaceGrant(auth, identities, actor, current.revision, next);
  let restored = false;
  return async () => {
    if (restored) return;
    const latest = await readGrant(auth, identities, actor);
    await replaceGrant(auth, identities, actor, latest.revision, current.capabilities);
    restored = true;
  };
}

export async function prepareBasicUsage(ctx, primitives) {
  requireSetup(basicUsageCases.includes(ctx.row.case_id), 'admin09_advanced_usage_recipe_missing');
  return prepareUsage(ctx, primitives, false);
}

export const setups = Object.freeze({admin_roles: prepareAdminRoles, usage: prepareBasicUsage});

