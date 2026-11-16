import {randomUUID} from 'node:crypto';
import {define, take, open, docPage, visible, pick, response, comments, path, other, goto} from './helpers.mjs';

async function share(ctx, session) {
  await session.page.getByRole('button', {name: 'Share', exact: true}).click();
  const dialog = session.page.getByRole('dialog').filter({has: session.page.getByText('Share links', {exact: true})});
  await visible(ctx, dialog, 'document_sharing_open');
  return dialog;
}
async function denied(ctx, fixture, document, actor) {
  for (const am of [ctx.am, other(ctx.am)]) {
    for (const suffix of ['', '/source', '/comments']) {
      const receipt = await ctx.request({actor, am, method: 'GET', path: path(document.id, suffix), allowed: [404]});
      ctx.assert(receipt.status === 404, 'document_revocation_protected_read');
    }
  }
}
async function recipientPermissions(ctx, fixture, session, permission, token) {
  const panel = await comments(ctx, session);
  ctx.assert(await session.page.getByRole('textbox', {name: 'Document title', exact: true}).getAttribute('readonly') !== null,
    'document_recipient_cannot_edit');
  ctx.assert(await session.page.getByRole('button', {name: 'Share', exact: true}).count() === 0,
    'document_recipient_cannot_manage_grants');
  if (permission === 'read') {
    ctx.assert(await panel.getByRole('textbox', {name: 'Add a comment'}).count() === 0, 'view_permission_no_comment_form');
  } else {
    const text = `Permitted comment ${randomUUID()}`;
    await panel.getByRole('textbox', {name: 'Add a comment'}).fill(text);
    const receipt = await response(session, 'POST', path(fixture.document.id, '/comments'), () =>
      panel.getByRole('button', {name: 'Comment', exact: true}).click());
    ctx.assert(receipt.status() === 201, 'comment_permission_allows_comment');
    const result = await receipt.json();
    ctx.assert(result.comment.body === text && result.comment.author_subject === fixture.actors.recipient.subject
      && result.comment.document_revision === fixture.document.source_revision, 'shared_comment_attribution');
  }
  // Real browser fetches traverse the same authenticated ingress proxy. Tokens
  // stay in memory and headers; return only status, never token-bearing payloads.
  const statuses = await session.page.evaluate(async ({id, unrelated, token, permission}) => {
    const headers = {'Content-Type': 'application/json', 'Idempotency-Key': crypto.randomUUID()};
    if (token) headers['X-CMA-Document-Share-Token'] = token;
    const patch = await fetch(`/api/v1/documents/${id}`, {method: 'PATCH', headers,
      body: JSON.stringify({expected_revision: 1, title: 'Denied edit'})});
    const management = await fetch(`/api/v1/documents/${id}/shares`, {headers});
    const foreign = await fetch(`/api/v1/documents/${unrelated}/source`, {headers});
    let comment;
    if (permission === 'read') comment = (await fetch(`/api/v1/documents/${id}/comments`, {method: 'POST',
      headers: {...headers, 'Idempotency-Key': crypto.randomUUID()}, body: JSON.stringify({body: 'Denied comment', document_revision: 1})})).status;
    return {patch: patch.status, management: management.status, foreign: foreign.status, comment};
  }, {id: fixture.document.id, unrelated: fixture.unrelated.id, token, permission});
  ctx.assert(statuses.patch === 404 && statuses.management === 404 && statuses.foreign === 404
    && (permission !== 'read' || statuses.comment === 404), 'document_permission_and_resource_scope');
}

export const peopleCase = define('people-sharing', async ctx => {
  const variants = [];
  await ctx.step('step_1', async () => {
    for (const permission of ['read', 'comment']) {
      const fixture = await take(ctx), owner = await open(ctx, fixture.actors.user.actor);
      await docPage(ctx, owner, fixture.document);
      const dialog = await share(ctx, owner);
      await pick(owner.page, 'Recipient permission', permission === 'read' ? 'Can view' : 'Can comment');
      await dialog.getByRole('textbox', {name: 'Recipient user ID'}).fill(fixture.actors.recipient.subject);
      ctx.assert(await dialog.getByRole('button', {name: 'Add person', exact: true}).isEnabled(), 'document_recipient_selected');
      variants.push({fixture, owner, dialog, permission});
    }
  });
  await ctx.step('step_2', async () => {
    for (const item of variants) {
      const receipt = await response(item.owner, 'PUT', path(item.fixture.document.id,
        `/shares/${encodeURIComponent(item.fixture.actors.recipient.subject)}`), () =>
        item.dialog.getByRole('button', {name: 'Add person', exact: true}).click());
      ctx.assert(receipt.status() === 200, 'document_person_granted');
      await visible(ctx, item.dialog.getByRole('list', {name: 'People with access'}).getByText(item.fixture.actors.recipient.subject, {exact: true}),
        'document_person_listed');
    }
  });
  await ctx.step('step_3', async () => {
    for (const item of variants) {
      item.recipient = await open(ctx, item.fixture.actors.recipient.actor, other(ctx.am));
      await docPage(ctx, item.recipient, item.fixture.document);
      await recipientPermissions(ctx, item.fixture, item.recipient, item.permission);
    }
  });
  await ctx.step('step_4', async () => {
    for (const item of variants) {
      // Commenting advances metadata; refresh the owner's current revision before revoke.
      await item.dialog.getByRole('button', {name: 'Done', exact: true}).click();
      await item.owner.page.reload(); await docPage(ctx, item.owner, item.fixture.document);
      item.dialog = await share(ctx, item.owner);
      const row = item.dialog.getByRole('list', {name: 'People with access'}).getByRole('listitem')
        .filter({hasText: item.fixture.actors.recipient.subject});
      await visible(ctx, row, 'document_revoke_target');
      const receipt = await response(item.owner, 'DELETE', path(item.fixture.document.id,
        `/shares/${encodeURIComponent(item.fixture.actors.recipient.subject)}`), () =>
        row.getByRole('button', {name: 'Remove', exact: true}).click());
      ctx.assert(receipt.status() === 200, 'document_person_revoked');
      await row.waitFor({state: 'hidden'});
      await item.recipient.page.reload();
      await visible(ctx, item.recipient.page.getByRole('button', {name: 'Reload latest', exact: true}), 'document_revoked_ui_refused');
      ctx.assert(await item.recipient.page.getByRole('textbox', {name: 'Document content', exact: true}).count() === 0,
        'document_revoked_ui_no_content');
    }
  });
  await ctx.step('expected_result', async () => {
    for (const {fixture} of variants) await denied(ctx, fixture, fixture.document, fixture.actors.recipient.actor);
  });
});

export const linkCase = define('link-sharing', async ctx => {
  const variants = [];
  await ctx.step('step_1', async () => {
    for (const permission of ['read', 'comment']) {
      const fixture = await take(ctx), owner = await open(ctx, fixture.actors.user.actor);
      await docPage(ctx, owner, fixture.document); const dialog = await share(ctx, owner);
      await pick(owner.page, 'Recipient permission', permission === 'read' ? 'Can view' : 'Can comment');
      const receipt = await response(owner, 'POST', path(fixture.document.id, '/share-links'), () =>
        dialog.getByRole('button', {name: `Create ${permission === 'read' ? 'view' : 'comment'} link`, exact: true}).click());
      ctx.assert(receipt.status() === 201, 'document_link_created');
      const created = await receipt.json();
      ctx.assert(created.link.permission === permission && created.document.id === fixture.document.id, 'document_link_scope_created');
      await visible(ctx, dialog.getByRole('textbox', {name: 'Document share link'}), 'document_link_available_once');
      variants.push({fixture, owner, dialog, permission, linkId: created.link.id});
    }
  });
  await ctx.step('step_2', async () => {
    for (const item of variants) {
      const link = await item.dialog.getByRole('textbox', {name: 'Document share link'}).inputValue();
      await item.dialog.getByRole('button', {name: 'Copy link', exact: true}).click();
      await visible(ctx, item.dialog.getByRole('status').filter({hasText: 'Link copied.'}), 'document_link_copied');
      const copied = await item.owner.page.evaluate(() => navigator.clipboard.readText());
      ctx.defer('clear_document_share_clipboard', () => item.owner.page.evaluate(() => navigator.clipboard.writeText('')));
      ctx.assert(copied === link, 'document_clipboard_exact_link');
      const url = new URL(copied); item.token = new URLSearchParams(url.hash.slice(1)).get('token');
      ctx.assert(url.pathname === `/artifacts/${item.fixture.document.id}` && /^[a-f0-9]{64}$/.test(item.token ?? ''),
        'document_link_fragment_scope');
      item.recipient = await open(ctx, item.fixture.actors.recipient.actor, other(ctx.am));
      // Each actor has an isolated local proxy; keep the exact route/fragment,
      // changing only the loopback transport origin for that actor's session.
      await goto(item.recipient, url.pathname + url.hash);
      await visible(ctx, item.recipient.page.getByRole('textbox', {name: 'Document title', exact: true}), 'link_recipient_open');
      await recipientPermissions(ctx, item.fixture, item.recipient, item.permission, item.token);
      ctx.assert(!new URL(item.recipient.page.url()).hash, 'link_token_removed_from_location');
    }
  });
  await ctx.step('step_3', async () => {
    for (const item of variants) {
      await item.dialog.getByRole('button', {name: 'Done', exact: true}).click();
      await item.owner.page.reload(); await docPage(ctx, item.owner, item.fixture.document);
      item.dialog = await share(ctx, item.owner);
      const row = item.dialog.getByRole('list', {name: 'Active share links'}).getByRole('listitem');
      await visible(ctx, row, 'document_link_revoke_target');
      const receipt = await response(item.owner, 'DELETE', path(item.fixture.document.id, `/share-links/${item.linkId}`), () =>
        row.getByRole('button', {name: 'Revoke', exact: true}).click());
      ctx.assert(receipt.status() === 200, 'document_link_revoked');
      await row.waitFor({state: 'hidden'});
      await item.recipient.page.reload();
      await visible(ctx, item.recipient.page.getByRole('button', {name: 'Reload latest', exact: true}), 'revoked_link_ui_refused');
      ctx.assert(await item.recipient.page.getByRole('textbox', {name: 'Document content', exact: true}).count() === 0,
        'revoked_link_ui_no_content');
    }
  });
  await ctx.step('expected_result', async () => {
    for (const item of variants) {
      for (const am of [ctx.am, other(ctx.am)]) {
        await item.recipient.switchAM(am);
        const statuses = await item.recipient.page.evaluate(async ({id, token}) => {
          const results = [];
          for (const suffix of ['', '/source', '/comments']) results.push((await fetch(`/api/v1/documents/${id}${suffix}`,
            {headers: {'X-CMA-Document-Share-Token': token}})).status);
          return results;
        }, {id: item.fixture.document.id, token: item.token});
        ctx.assert(statuses.every(status => status === 404), 'revoked_link_refused_on_both_ingresses');
      }
    }
  });
});
