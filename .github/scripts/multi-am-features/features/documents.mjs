import {randomUUID, randomBytes, createHash} from 'node:crypto';
import {check} from '../cma/core.mjs';

const digest = value => createHash('sha256').update(value).digest('hex');
const root = '/api/v1/documents';

// Documents have no AM ownership lease. Each write is observed through the
// other real AM; separate subjects prove that a global operator has no implicit
// document access. Records are owned by this disposable fixture's database.
export async function documentCases({owner, recipient, ownerId, recipientId, runCase}) {
  const name = 'CI document ' + randomUUID();
  let document;
  const path = suffix => root + '/' + encodeURIComponent(document.id) + suffix;
  const request = async (client, am, method, route, options) =>
    (await client.request(am, method, route, options)).body;
  const read = async am => request(owner, am, 'GET', path(''));
  const mutation = async (client, am, method, suffix, body, allowed = [200]) =>
    request(client, am, method, path(suffix), {body, key: randomUUID(), allowed});

  await runCase('user.documents_create_edit_conflict', async () => {
    const markdown = '# ' + name + '\n\nOriginal source.\n';
    const body = {title: name, markdown};
    const key = randomUUID();
    document = await request(owner, 'a', 'POST', root, {body, key, allowed: [201]});
    check(typeof document.id === 'string' && document.id.length > 0
      && document.owner_subject === ownerId && document.access === 'owner', 'document_create_identity');
    check(document.sha256 === digest(markdown) && document.size_bytes === Buffer.byteLength(markdown),
      'document_create_digest');
    // Deliberate same-intent replay after a known successful receipt.
    const replay = await request(owner, 'b', 'POST', root, {body, key, allowed: [201]});
    check(replay.id === document.id && replay.revision === document.revision, 'document_duplicate_create');
    const listing = await request(owner, 'b', 'GET', root + '?scope=owned&q=' + encodeURIComponent(name));
    check(listing.items.filter(row => row.id === document.id).length === 1, 'document_search_identity');
    await request(recipient, 'b', 'GET', path(''), {allowed: [404]});
    const original = await read('b');
    const edited = markdown + '\n## Preserved formatting\n\n- **Strong**\n- `code`\n';
    document = await mutation(owner, 'b', 'PATCH', '', {
      expected_revision: original.revision, title: name + ' edited', markdown: edited});
    check(document.revision > original.revision && document.source_revision > original.source_revision,
      'document_update_revision');
    const seen = await read('a');
    check(seen.revision === document.revision && seen.sha256 === digest(edited)
      && seen.title === name + ' edited', 'document_peer_update');
    await mutation(owner, 'a', 'PATCH', '', {
      expected_revision: original.revision, markdown: 'stale overwrite'}, [409]);
    const source = await request(owner, 'b', 'GET', path('/source'));
    check(source.id === document.id && source.revision === document.source_revision
      && source.markdown === edited, 'document_stale_write_changed_source');
    const old = await request(owner, 'a', 'GET', path('/source?revision=' + original.source_revision));
    check(old.markdown === markdown && old.revision === original.source_revision, 'document_old_source_changed');
    const revisions = await request(owner, 'b', 'GET', path('/revisions'));
    check(revisions.items.some(row => row.revision === original.source_revision && row.sha256 === digest(markdown))
      && revisions.items.some(row => row.revision === document.source_revision && row.sha256 === digest(edited)),
      'document_revision_history');
  });

  await runCase('user.documents_grant_comment_revoke', async () => {
    const before = await read('a');
    document = await mutation(owner, 'a', 'PUT', '/shares/' + encodeURIComponent(recipientId), {
      expected_revision: before.revision, permission: 'comment'});
    try {
      const shared = await request(recipient, 'b', 'GET', path(''));
      check(shared.id === document.id && shared.access === 'comment' && shared.owner_subject === ownerId,
        'document_recipient_scope');
      const listing = await request(recipient, 'b', 'GET', root + '?scope=shared&q=' + encodeURIComponent(name));
      check(listing.items.some(row => row.id === document.id && row.access === 'comment'), 'document_shared_listing');
      await mutation(recipient, 'b', 'PATCH', '', {expected_revision: shared.revision, title: 'forbidden'}, [404]);
      await request(recipient, 'b', 'GET', path('/shares'), {allowed: [404]});
      const body = 'Comment ' + randomUUID();
      const added = await mutation(recipient, 'b', 'POST', '/comments', {
        body, document_revision: shared.source_revision}, [201]);
      check(added.comment?.body === body && added.comment.author_subject === recipientId
        && added.comment.document_revision === shared.source_revision, 'document_comment_identity');
      const page = await request(owner, 'a', 'GET', path('/comments'));
      check(page.items.filter(row => row.id === added.comment.id && row.body === body).length === 1,
        'document_comment_peer_readback');
      const current = await read('a');
      await mutation(owner, 'a', 'DELETE', '/comments/' + encodeURIComponent(added.comment.id), {
        expected_revision: current.revision});
      const removed = await request(recipient, 'b', 'GET', path('/comments'));
      check(!removed.items.some(row => row.id === added.comment.id), 'document_comment_remove_readback');
    } finally {
      const current = await read('b');
      await mutation(owner, 'b', 'DELETE', '/shares/' + encodeURIComponent(recipientId), {
        expected_revision: current.revision});
    }
    for (const am of ['a', 'b']) {
      await request(recipient, am, 'GET', path(''), {allowed: [404]});
      await request(recipient, am, 'GET', path('/source'), {allowed: [404]});
    }
    const shares = await request(owner, 'a', 'GET', path('/shares'));
    check(!shares.some(row => row.subject === recipientId), 'document_grant_revoke_readback');
  });

  await runCase('user.documents_link_revoke', async () => {
    const token = randomBytes(32).toString('hex');
    const before = await read('a');
    const created = await mutation(owner, 'a', 'POST', '/share-links', {
      expected_revision: before.revision, permission: 'read', token}, [201]);
    check(created.document.id === document.id && typeof created.link?.id === 'string', 'document_link_identity');
    try {
      const shared = await request(recipient, 'b', 'GET', path(''), {documentShareToken: token});
      check(shared.access === 'read' && shared.id === document.id && shared.owner_subject === ownerId,
        'document_link_scope');
      const source = await request(recipient, 'b', 'GET', path('/source'), {documentShareToken: token});
      check(source.id === document.id && digest(source.markdown) === shared.sha256, 'document_link_source');
      await recipient.request('b', 'POST', path('/comments'), {body: {body: 'forbidden'}, key: randomUUID(),
        documentShareToken: token, allowed: [404]});
      const links = await request(owner, 'b', 'GET', path('/share-links'));
      check(links.some(row => row.id === created.link.id) && !JSON.stringify(links).includes(token),
        'document_link_metadata_discloses_token');
    } finally {
      const current = await read('b');
      await mutation(owner, 'b', 'DELETE', '/share-links/' + encodeURIComponent(created.link.id), {
        expected_revision: current.revision});
    }
    for (const am of ['a', 'b']) await request(recipient, am, 'GET', path(''), {
      documentShareToken: token, allowed: [404]});
  });
  return read('b');
}
