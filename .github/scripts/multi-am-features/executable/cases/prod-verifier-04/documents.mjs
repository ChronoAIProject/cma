import {caseOf, equal, hash, key, otherAM, pages, request, segment, take} from './common.mjs';

export const documentCases = [34,35].map(number => caseOf(number, 'global', ['api-fixtures'], async ctx => {
  let fixture, path, initial, receipt, deletionKey, deletionBody, target;
  async function revisions() {
    const actual = await pages(ctx, path + '/revisions', {am: otherAM(ctx.am)});
    ctx.assert(actual.length === fixture.revisions.length, 'document_revision_count');
    for (const expected of fixture.revisions) {
      const row = actual.find(value => value.revision === expected.revision);
      ctx.assert(row && row.title === expected.title && row.sha256 === hash(expected.markdown)
        && row.size_bytes === Buffer.byteLength(expected.markdown), 'document_revision_hash');
      const source = await request(ctx, 'GET', path + '/source?revision=' + expected.revision,
        undefined, {am: otherAM(ctx.am)});
      ctx.assert(source.body.id === fixture.documentId && source.body.revision === expected.revision
        && source.body.markdown === expected.markdown, 'document_revision_source');
    }
    return actual;
  }
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'documents'); path = '/api/v1/documents/' + segment(fixture.documentId);
    ctx.assert(fixture.revisions?.length >= 2 && fixture.comments?.length >= 2, 'document_fixture_history');
    initial = await revisions();
    const doc = (await request(ctx, 'GET', path)).body;
    target = fixture.comments.find(value => value.author_subject === fixture.owner);
    ctx.assert(Boolean(target), 'document_owned_comment_required');
    const comments = await pages(ctx, path + '/comments');
    ctx.assert(fixture.comments.every(expected => comments.some(actual => equal(actual, expected))), 'document_seed_comments');
    deletionBody = {expected_revision: doc.revision}; deletionKey = key();
    // Capture a pending public mutation before dispatch; namespace cleanup is
    // registered by take even if the response is lost.
    receipt = await request(ctx, 'DELETE', path + '/comments/' + segment(target.id), deletionBody, {key: deletionKey});
    ctx.assert(receipt.body.id === fixture.documentId && receipt.body.revision === doc.revision + 1,
      'document_comment_deleted_revision');
  });
  await ctx.step('step_2', async () => {
    if (number === 34) {
      const replay = await request(ctx, 'DELETE', path + '/comments/' + segment(target.id), deletionBody,
        {key: deletionKey, am: otherAM(ctx.am)});
      ctx.assert(equal(replay.body, receipt.body), 'document_delete_same_intent_receipt');
    } else {
      const result = await request(ctx, 'GET', path + '/revisions');
      ctx.assert(Array.isArray(result.body.items) && result.body.items.length > 0, 'document_exact_revision_page');
      for (const row of result.body.items) ctx.assert(initial.some(value => equal(value, row)), 'document_exact_committed_revision');
    }
  });
  await ctx.step('step_3', async () => {
    const denied = await request(ctx, 'DELETE', path + '/comments/' + segment(fixture.comments.find(c => c.id !== target.id).id),
      {expected_revision: receipt.body.revision}, {actor: 'outsider', key: key(), allowed: [403,404]});
    ctx.assert(typeof denied.body.code === 'string' && !('owner_subject' in denied.body), 'document_unauthorized_delete_refused');
    const stale = await request(ctx, 'DELETE', path + '/comments/' + segment(fixture.comments.find(c => c.id !== target.id).id),
      deletionBody, {key: key(), am: otherAM(ctx.am), allowed: [409]});
    ctx.assert(typeof stale.body.code === 'string', 'document_stale_revision_refused');
    const deniedRead = await request(ctx, 'GET', path + '/revisions', undefined, {actor: 'outsider', allowed: [403,404]});
    ctx.assert(!('items' in deniedRead.body), 'document_unauthorized_history_refused');
  });
  await ctx.step('expected_result', async () => {
    const remaining = await pages(ctx, path + '/comments', {am: otherAM(ctx.am)});
    ctx.assert(!remaining.some(value => value.id === target.id)
      && equal(remaining.slice().sort((a,b) => a.id.localeCompare(b.id)),
        fixture.comments.filter(value => value.id !== target.id).slice().sort((a,b) => a.id.localeCompare(b.id))),
    'document_only_target_comment_deleted');
    ctx.assert(equal(await revisions(), initial), 'document_source_history_unchanged');
  });
}));
