import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {check, hash, root, path, other} from './helpers.mjs';

// The bridge supplies identities and custody, never expected feature outcomes.
// No direct database connection, privileged writes, or production fixture fallback.
export async function prepareDocuments(ctx, dependencies, variant = 'basic') {
  const {identities, disposableDatabase} = dependencies ?? {};
  if (disposableDatabase?.scope !== 'LOCAL_DISPOSABLE_TWO_AM'
    || disposableDatabase.database_disposable !== true
    || typeof disposableDatabase.reserve !== 'function') throw new Blocked('document_disposable_database_required');
  const actors = {};
  for (const name of ['user', 'recipient', 'outsider']) {
    const identity = identities?.[name];
    if (!identity?.actor || !identity.subject) throw new Blocked('document_fixture_identity_missing');
    actors[name] = {actor: identity.actor, subject: identity.subject};
  }
  if (new Set(Object.values(actors).map(value => value.subject)).size !== 3) throw new Blocked('document_distinct_subjects_required');
  const lease = await disposableDatabase.reserve({purpose: 'documents', caseId: ctx.row.case_id});
  if (lease?.parent_cleanup_barrier !== true || typeof lease.retainIntent !== 'function'
    || typeof lease.retainDocument !== 'function' || typeof lease.release !== 'function') {
    throw new Blocked('document_parent_cleanup_barrier_required');
  }
  let released = false;
  const cleanup = async () => {
    if (released) return;
    const receipt = await lease.release();
    check(receipt?.registered === true && receipt.parent_cleanup_barrier === true, 'document_cleanup_custody_unconfirmed');
    released = true;
    await ctx.observe('document_parent_custody', async () => ({registered: true, parent_cleanup_barrier: true,
      final_database_cleanup: 'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS'}));
  };
  ctx.defer('release_document_setup', cleanup);
  const prefix = `C275 docs ${randomUUID()}`;
  const documents = [];
  const create = async (owner, title, markdown = `${title}\n\nLocal fixture source.\n`) => {
    const key = randomUUID(), body = {title, markdown};
    await lease.retainIntent({actor: actors[owner].actor, method: 'POST', path: root, key, body});
    let receipt;
    try {
      receipt = await ctx.request({actor: actors[owner].actor, am: ctx.am, method: 'POST', path: root,
        key, body, allowed: [201]});
    } catch (error) {
      // Attribution failure may follow a committed create. Preserve it without replay.
      if (error.response?.status === 201 && error.response.body?.id) await lease.retainDocument(error.response.body.id);
      throw error;
    }
    const document = receipt.body;
    await lease.retainDocument(document.id);
    check(document.owner_subject === actors[owner].subject && document.access === 'owner'
      && document.sha256 === hash(markdown), 'document_setup_create_mismatch');
    documents.push(document);
    return document;
  };
  const grant = async (document, owner, viewer, permission) => {
    const current = (await ctx.request({actor: actors[owner].actor, am: other(ctx.am), method: 'GET', path: path(document.id)})).body;
    return (await ctx.request({actor: actors[owner].actor, am: ctx.am, method: 'PUT',
      path: path(document.id, `/shares/${encodeURIComponent(actors[viewer].subject)}`), key: randomUUID(),
      body: {expected_revision: current.revision, permission}})).body;
  };
  const fixture = {scope: 'LOCAL_DISPOSABLE_TWO_AM', cleanupRegistered: true, cleanup,
    actors, lease, prefix, create, grant, documents};
  if (variant !== 'empty') {
    fixture.document = await create('user', `${prefix} owned`);
    fixture.unrelated = await create('outsider', `${prefix} hidden`);
  }
  if (variant === 'browse') {
    fixture.owned = [fixture.document];
    for (let index = 1; index < 26; index++) fixture.owned.push(await create('user', `${prefix} owned ${index}`));
    fixture.shared = await create('recipient', `${prefix} shared`);
    await grant(fixture.shared, 'recipient', 'user', 'read');
  }
  if (variant === 'comments') {
    fixture.comments = [];
    for (let index = 0; index < 26; index++) {
      const body = `${prefix} comment ${String(index).padStart(2, '0')}`;
      const receipt = await ctx.request({actor: actors.user.actor, am: index % 2 ? other(ctx.am) : ctx.am,
        method: 'POST', path: path(fixture.document.id, '/comments'), key: randomUUID(), allowed: [201],
        body: {body, document_revision: fixture.document.source_revision}});
      fixture.comments.push(receipt.body.comment);
    }
    await grant(fixture.document, 'user', 'recipient', 'read');
  }
  return fixture;
}

export const setups = Object.fromEntries(['basic', 'empty', 'browse', 'comments'].map(variant =>
  [`documents-${variant}`, (ctx, dependencies) => prepareDocuments(ctx, dependencies, variant)]));
