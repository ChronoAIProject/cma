import {randomUUID} from 'node:crypto';
import {define, take, open, docPage, comments, visible, path, other, loseAcknowledgment, read} from './helpers.mjs';

export const commentsReadCase = define('comments-read', async ctx => {
  let fixture, session, panel;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'documents-comments'); session = await open(ctx, fixture.actors.recipient.actor);
    await docPage(ctx, session, fixture.document); panel = await comments(ctx, session);
    await visible(ctx, panel.locator('.document-comment').first(), 'document_comment_visible');
  });
  await ctx.step('step_2', async () => {
    await visible(ctx, panel.getByRole('button', {name: 'More comments', exact: true}), 'comment_pagination_available');
    const pending = session.page.waitForResponse(value => new URL(value.url()).pathname === path(fixture.document.id, '/comments')
      && new URL(value.url()).searchParams.has('cursor'));
    await Promise.all([pending, panel.getByRole('button', {name: 'More comments', exact: true}).click()]);
    await panel.getByRole('button', {name: 'More comments', exact: true}).waitFor({state: 'hidden'});
    ctx.assert(await panel.locator('.document-comment').count() === 26, 'all_comments_loaded');
  });
  await ctx.step('step_3', async () => {
    const articles = await panel.locator('.document-comment').evaluateAll(nodes => nodes.map(node => ({
      author: node.querySelector('strong')?.textContent, version: node.querySelector('small')?.textContent,
      body: node.querySelector('p')?.textContent})));
    ctx.assert(new Set(articles.map(row => row.body)).size === 26, 'comments_unique');
    for (const expected of fixture.comments) ctx.assert(articles.some(row => row.body === expected.body
      && row.author === expected.author_subject && row.version === `Version ${expected.document_revision}`), 'comment_attribution');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(await panel.getByRole('textbox', {name: 'Add a comment'}).count() === 0, 'readonly_comment_form_absent');
    const forbidden = await ctx.request({actor: fixture.actors.recipient.actor, am: other(ctx.am), method: 'POST',
      path: path(fixture.document.id, '/comments'), key: randomUUID(), body: {body: 'Denied', document_revision: 1}, allowed: [404]});
    ctx.assert(forbidden.status === 404, 'readonly_comment_admission_refused');
    const hidden = await ctx.request({actor: fixture.actors.recipient.actor, am: ctx.am, method: 'GET',
      path: path(fixture.unrelated.id, '/comments'), allowed: [404]});
    ctx.assert(hidden.status === 404, 'unrelated_comments_hidden');
    await session.switchAM(other(ctx.am)); await session.page.reload();
    await docPage(ctx, session, fixture.document); panel = await comments(ctx, session);
    ctx.assert(await panel.getByRole('textbox', {name: 'Add a comment'}).count() === 0, 'peer_readonly_comment_form_absent');
  });
});

export const commentCase = define('comment', async ctx => {
  let fixture, session, panel, fault, accepted;
  const text = `Comment once ${randomUUID()}`;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx); await fixture.grant(fixture.document, 'user', 'recipient', 'comment');
    session = await open(ctx, fixture.actors.recipient.actor);
    await docPage(ctx, session, fixture.document); panel = await comments(ctx, session);
    await visible(ctx, panel.getByRole('textbox', {name: 'Add a comment'}), 'comment_permission_form');
    fault = await loseAcknowledgment(ctx, session, 'POST', path(fixture.document.id, '/comments'));
  });
  await ctx.step('step_2', async () => {
    const input = panel.getByRole('textbox', {name: 'Add a comment'});
    await input.fill('€'.repeat(3000));
    await visible(ctx, panel.getByRole('alert').filter({hasText: 'Keep your comment within 8 KiB.'}), 'comment_byte_limit');
    ctx.assert(await panel.getByRole('button', {name: 'Comment', exact: true}).isDisabled(), 'oversize_comment_disabled');
    await input.fill(text);
    ctx.assert(await input.inputValue() === text && await panel.getByRole('button', {name: 'Comment', exact: true}).isEnabled(),
      'bounded_comment_enabled');
  });
  await ctx.step('step_3', async () => {
    await panel.getByRole('button', {name: 'Comment', exact: true}).click();
    await visible(ctx, panel.getByRole('button', {name: 'Retry comment', exact: true}), 'comment_unknown_outcome_retained');
    ctx.assert(await panel.getByRole('textbox', {name: 'Add a comment'}).isDisabled(), 'comment_unknown_intent_locked');
    await session.switchAM(other(ctx.am));
    await panel.getByRole('button', {name: 'Retry comment', exact: true}).click();
    await visible(ctx, panel.locator('.document-comment p').filter({hasText: text}), 'comment_retry_visible');
    accepted = await fault.verify();
    await session.page.reload(); await docPage(ctx, session, fixture.document); panel = await comments(ctx, session);
    await visible(ctx, panel.locator('.document-comment p').filter({hasText: text}), 'comment_reload_visible');
  });
  await ctx.step('expected_result', async () => {
    const current = await read(ctx, fixture.document, fixture.actors.user.actor);
    ctx.assert(accepted.comment.author_subject === fixture.actors.recipient.subject
      && accepted.comment.document_revision === current.source_revision, 'comment_author_revision');
    for (const am of [ctx.am, other(ctx.am)]) {
      const listing = (await ctx.request({actor: fixture.actors.user.actor, am, method: 'GET',
        path: path(fixture.document.id, '/comments')})).body;
      ctx.assert(listing.items.length === 1 && listing.items[0].id === accepted.comment.id
        && listing.items[0].body === text, 'comment_retry_exactly_once');
    }
    ctx.assert(await panel.locator('.document-comment').count() === 1, 'comment_ui_no_duplicate');
  });
});
