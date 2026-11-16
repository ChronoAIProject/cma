import {randomUUID} from 'node:crypto';
import {check, equal, id, other, request, read, P} from './common.mjs';

// Public authoring only. The global settings lease is held by the parent, and
// profile retirement runs after case-level restoration of deployment defaults.
export async function publicDefaultProfile(ctx, kind, content, owner) {
  const name = 'admin01-default-' + randomUUID(), key = randomUUID();
  const base = '/api/v1/' + kind + '-profiles';
  let profileId, attempted = false, cleanupJob;
  const cleanup = () => cleanupJob ??= (async () => {
    if (!attempted) return;
    check(profileId, 'admin_default_profile_create_unknown');
    const settings = await read(ctx, P + '/settings', other(ctx.am));
    check(settings.settings.defaults[kind + '_profile'].id !== profileId, 'admin_default_profile_still_selected');
    const path = base + '/' + id(profileId);
    const current = (await request(ctx, 'GET', path, undefined, {actor: 'user', am: other(ctx.am)})).body;
    check(current.id === profileId && current.name === name && current.owner === owner,
      'admin_default_profile_cleanup_identity');
    if (!current.retired_at) await request(ctx, 'POST', path + '/retire', undefined,
      {actor: 'user', key: key + '-retire'});
    const final = (await request(ctx, 'GET', path, undefined, {actor: 'user'})).body;
    check(final.id === profileId && !!final.retired_at, 'admin_default_profile_retirement_unconfirmed');
  })();
  ctx.defer('admin_default_' + kind + '_profile_cleanup', cleanup);
  const body = {name, description: 'Disposable local default-profile fixture', content};
  await ctx.observe('admin_default_profile_intent', async () => ({kind, key, body, actor: 'user'}));
  attempted = true;
  let created;
  try {
    created = (await request(ctx, 'POST', base, body, {actor: 'user', key, allowed: [201]})).body;
    profileId = created?.id;
  } catch (error) {profileId = error.response?.body?.id; throw error;}
  check(profileId && created.name === name && created.owner === owner && created.visibility === 'private'
    && Number.isSafeInteger(created.draft_version)
    && Object.entries(content).every(([key, value]) => equal(created.draft?.[key], value)),
  'admin_default_profile_created_identity');
  const path = base + '/' + id(profileId);
  const revision = (await request(ctx, 'POST', path + '/revisions', {draft_version: created.draft_version},
    {actor: 'user', key: key + '-publish', allowed: [201]})).body;
  check(revision.profile_id === profileId && Number.isSafeInteger(revision.revision) && revision.revision > 0
    && equal(revision.content, created.draft),
    'admin_default_profile_revision_identity');
  const actual = (await request(ctx, 'GET', path + '/revisions/' + revision.revision, undefined,
    {actor: 'user', am: other(ctx.am)})).body;
  check(actual.profile_id === profileId && actual.revision === revision.revision
    && actual.digest === revision.digest && equal(actual.content, revision.content), 'admin_default_profile_revision_readback');
  const visible = (await request(ctx, 'PUT', path + '/visibility', {visibility: 'public'},
    {actor: 'user', key: key + '-visibility'})).body;
  check(visible.id === profileId && visible.visibility === 'public', 'admin_default_profile_not_public');
  return {reference: {id: profileId, revision: revision.revision}, name, cleanup};
}
