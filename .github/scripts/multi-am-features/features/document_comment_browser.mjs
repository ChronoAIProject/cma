import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, id, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;

export async function documentCommentBrowser({page, user, ingress, documentId, ownerId, recordFailure}) {
  check(id(documentId), 'browser_document_comment_lineage_required');
  const path = '/api/v1/documents/' + documentId;
  const comments = path + '/comments';
  const peer = other(ingress);
  const text = 'Fixture comment ' + randomUUID();
  const readDocument = async am => (await user.request(am, 'GET', path)).body;
  const readComment = async am => {
    const {body} = await user.request(am, 'GET', comments);
    check(Array.isArray(body?.items) && !body.next, 'browser_document_comment_list_unbound');
    const matches = body.items.filter(row => row.body === text);
    check(matches.length <= 1, 'browser_document_comment_duplicated');
    return matches[0];
  };
  const document = await readDocument(peer);
  check(document.id === documentId && document.owner_subject === ownerId && document.access === 'owner'
    && Number.isSafeInteger(document.source_revision), 'browser_document_comment_revision_unbound');
  const endpoint = url => url.pathname === comments;
  const isPost = request => request.method() === 'POST' && endpoint(new URL(request.url()));
  const intent = async request => ({key: await request.headerValue('idempotency-key'), body: request.postDataJSON()});
  const button = name => page.getByRole('button', {name, exact: true});
  const panel = page.getByRole('complementary', {name: 'Document comments', exact: true});
  const field = panel.getByRole('textbox', {name: 'Add a comment', exact: true});
  const showComment = async () => {
    const article = panel.getByRole('article').filter({hasText: text});
    await article.getByText(text, {exact: true}).waitFor({timeout});
    await article.getByText('You', {exact: true}).waitFor({timeout});
    await article.getByText('Version ' + document.source_revision, {exact: true}).waitFor({timeout});
    check(await article.count() === 1, 'browser_document_comment_rendered_twice');
  };

  let original, first, transportFailure;
  const dropReply = async route => {
    if (!isPost(route.request())) return route.fallback();
    try {
      check(!first, 'browser_document_comment_submitted_twice');
      first = await intent(route.request());
      check(first.key && equal(first.body, {body: text, document_revision: document.source_revision}),
        'browser_document_comment_original_intent_changed');
      const response = await route.fetch({timeout, maxRedirects: 0, maxRetries: 0});
      check(response.status() === 201, 'browser_document_comment_original_submit_refused');
      original = await response.json();
      check(id(original.comment?.id) && original.comment.body === text
        && original.comment.author_subject === ownerId && original.comment.document_revision === document.source_revision,
      'browser_document_comment_original_receipt_unbound');
    } catch (error) {
      transportFailure = error;
    } finally {
      await route.abort('failed');
    }
  };

  await withRestoration(async () => {
    await page.goto('/artifacts/' + documentId, {waitUntil: 'domcontentloaded', timeout});
    await button('Comments').click({timeout});
    await field.fill('界'.repeat(3000), {timeout});
    await panel.getByRole('alert').getByText('Keep your comment within 8 KiB.', {exact: true}).waitFor({timeout});
    check(await button('Comment').isDisabled(), 'browser_document_comment_oversized_submit_enabled');
    await field.fill(text, {timeout});
    await page.route(endpoint, dropReply);
    try {
      await button('Comment').click({timeout});
      await button('Retry comment').waitFor({timeout});
      if (transportFailure) throw transportFailure;
      check(equal(await readComment(peer), original.comment), 'browser_document_comment_commit_missing_on_peer');
    } finally {
      await page.unroute(endpoint, dropReply);
    }
    const [retry] = await Promise.all([
      page.waitForResponse(response => isPost(response.request()), {timeout}),
      button('Retry comment').click({timeout}),
    ]);
    check(retry.status() === 201 && equal(await intent(retry.request()), first)
      && equal(await retry.json(), original), 'browser_document_comment_retry_changed_receipt');
    await showComment();
    await page.reload({waitUntil: 'domcontentloaded', timeout});
    await button('Comments').click({timeout});
    await showComment();
    for (const am of [ingress, peer]) {
      check(equal(await readComment(am), original.comment), 'browser_document_comment_reloaded_receipt_changed');
    }
  }, async () => {
    const observed = await readComment(peer);
    if (observed) {
      check(id(observed.id) && observed.author_subject === ownerId
        && observed.document_revision === document.source_revision
        && (!id(original?.comment?.id) || observed.id === original.comment.id), 'browser_document_comment_cleanup_unbound');
      const latest = await readDocument(peer);
      await user.request(peer, 'DELETE', comments + '/' + encodeURIComponent(observed.id), {key: randomUUID(),
        body: {expected_revision: latest.revision}});
    }
    for (const am of [ingress, peer]) check(!await readComment(am), 'browser_document_comment_cleanup_unconfirmed');
  }, recordFailure);
}
