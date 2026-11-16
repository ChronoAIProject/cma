import {randomUUID} from 'node:crypto';
import {check, read, request, id, P, other} from './common.mjs';

// A valid published alternate avoids installing an invented default reference.
// Profile retirement occurs after case-level settings restoration. No unknown
// mutation is retried or rekeyed; final parent disposal owns retained audit rows.
export async function publishedProfile(ctx) {
  const name = 'coverage-admin-patch-' + randomUUID();
  let profileId, attempted = false, retired = false, cleaned = false;
  const cleanup = async () => {
    if (cleaned || !attempted) return;
    check(profileId, 'admin_profile_unknown_create_requires_parent_cleanup');
    const settings = await read(ctx, P + '/settings', other(ctx.am));
    check(settings.settings.defaults.agent_profile.id !== profileId, 'admin_profile_still_default');
    const path = '/api/v1/agent-profiles/' + id(profileId);
    const before = await read(ctx, path, other(ctx.am));
    check(before.id === profileId && before.name === name, 'admin_profile_cleanup_identity');
    if (!before.retired_at) {
      check(!retired, 'admin_profile_retire_unknown_requires_parent_cleanup');
      retired = true;
      await request(ctx, 'POST', path + '/retire', undefined, {key: name + '-retire'});
    }
    const after = await read(ctx, path);
    check(after.id === profileId && !!after.retired_at, 'admin_profile_retirement_unconfirmed');
    cleaned = true;
  };
  ctx.defer('admin_setup_profile_retire', cleanup);
  await ctx.observe('admin_profile_database_finality', async () => ({
    final_database_cleanup: 'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS',
  }));
  attempted = true;
  let created;
  try {
    created = (await request(ctx, 'POST', '/api/v1/agent-profiles', {
      name, description: 'Disposable ADMIN patch regression fixture', content: {
        thinking_effort: 'low', permission_level: 'guarded', skills: [], system_prompt: '',
      },
    }, {key: name, allowed: [201]})).body;
    profileId = created?.id;
  } catch (error) {
    profileId = error.response?.body?.id;
    throw error;
  }
  check(profileId && created.name === name && Number.isSafeInteger(created.draft_version),
    'admin_profile_create_shape');
  const revision = (await request(ctx, 'POST', '/api/v1/agent-profiles/' + id(profileId) + '/revisions',
    {draft_version: created.draft_version}, {key: name + '-publish', allowed: [201]})).body;
  check(revision.profile_id === profileId && Number.isSafeInteger(revision.revision)
    && revision.revision > 0, 'admin_profile_publish_identity');
  const actual = await read(ctx, '/api/v1/agent-profiles/' + id(profileId)
    + '/revisions/' + revision.revision, other(ctx.am));
  check(actual.profile_id === profileId && actual.revision === revision.revision
    && actual.digest === revision.digest, 'admin_profile_revision_readback');
  return {reference: {id: profileId, revision: revision.revision}, cleanup};
}
