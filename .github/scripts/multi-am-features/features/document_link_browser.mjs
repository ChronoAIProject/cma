import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, hash, id, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;
const shareHeader = 'x-cma-document-share-token';

export async function documentLinkBrowser({page, ingress, owner, recipient, ownerId, recipientId,
  recipientCookie, documentId, recordFailure}) {
  check([documentId, ownerId, recipientId].every(id) && ownerId !== recipientId
    && typeof recipientCookie === 'string' && recipientCookie.length > 0, 'document_link_browser_fixture_invalid');
  const peer = other(ingress);
  const path = '/api/v1/documents/' + documentId;
  const links = path + '/share-links';
  const route = '/artifacts/' + documentId;
  const visit = path => page.goto(path, {waitUntil: 'domcontentloaded', timeout});
  const read = async (am, suffix = '') => (await owner.request(am, 'GET', path + suffix)).body;
  const initial = await read(peer);
  check(initial.id === documentId && initial.owner_subject === ownerId && initial.access === 'owner'
    && (await read(peer, '/share-links')).length === 0, 'document_link_browser_original_unbound');
  for (const am of [ingress, peer]) {
    const {body} = await recipient.request(am, 'GET', path, {allowed: [404]});
    check(body?.code === 'document_not_found', 'document_link_browser_recipient_already_authorized');
  }
  await visit(route);
  const {origin, hostname} = new URL(page.url());
  const context = page.context();
  const cookies = (await context.cookies(origin)).filter(cookie => cookie.name === 'cma_session'
    && cookie.domain === hostname && cookie.path === '/');
  const [originalCookie] = cookies;
  check(cookies.length === 1 && originalCookie.httpOnly && originalCookie.value !== recipientCookie,
    'document_link_browser_cookie_unbound');
  const switchTo = async (cookie, subject) => {
    await context.addCookies([cookie]);
    const response = await page.request.get('/auth/session', {maxRedirects: 0, timeout});
    const identity = await response.json();
    check(response.ok() && identity.authenticated === true && identity.subject === subject,
      'document_link_browser_actor_unbound');
  };
  const ownerSession = () => switchTo(originalCookie, ownerId);
  const recipientSession = () => switchTo({...originalCookie, value: recipientCookie}, recipientId);
  const button = (scope, name) => scope.getByRole('button', {name, exact: true});
  const dialog = page.getByRole('dialog', {name: /^Share /});
  const listed = dialog.getByRole('list', {name: 'Active share links', exact: true});
  const observe = (method, expectedPath) => page.waitForResponse(response => {
    const url = new URL(response.url());
    return url.origin === origin && url.pathname === expectedPath && response.request().method() === method;
  }, {timeout});
  const drainDialog = async action => {
    const [shares, shareLinks] = await Promise.all([
      observe('GET', path + '/shares'), observe('GET', links), action(),
    ]);
    for (const response of [shares, shareLinks]) {
      check(response.ok() && await response.finished() === null, 'document_link_browser_dialog_read_failed');
    }
  };
  const openDialog = () => drainDialog(() => button(page, 'Share').click({timeout}));
  let token, linkId, attempted = false;
  const observeCreation = request => {
    if (request.method() === 'POST' && request.url() === origin + links) {
      attempted = true;
      token = request.postDataJSON()?.token;
    }
  };
  page.on('request', observeCreation);
  await withRestoration(async () => {
    await openDialog();
    await drainDialog(async () => {
      const [response] = await Promise.all([
        observe('POST', links), button(dialog, 'Create view link').click({timeout}),
      ]);
      const request = response.request(), body = request.postDataJSON();
      const created = await response.json();
      linkId = created?.link?.id;
      check(response.status() === 201 && /^[a-f0-9]{64}$/.test(token ?? '') && id(linkId)
        && body.expected_revision === initial.revision && body.permission === 'read'
        && await request.headerValue('idempotency-key') && created.link.permission === 'read'
        && created.document.id === documentId && created.document.revision === initial.revision + 1
        && created.document.source_revision === initial.source_revision, 'document_link_browser_create_unbound');
    });
    const listing = await read(peer, '/share-links');
    check(listing.length === 1 && listing[0].id === linkId && listing[0].permission === 'read'
      && !JSON.stringify(listing).includes(token), 'document_link_browser_peer_listing_unbound');
    const copy = listed.getByRole('textbox', {name: 'Document share link', exact: true});
    const link = origin + route + '#token=' + token;
    check(await copy.inputValue({timeout}) === link, 'document_link_browser_clipboard_source_unbound');
    await context.grantPermissions(['clipboard-read', 'clipboard-write'], {origin});
    try {
      await button(listed, 'Copy link').click({timeout});
      await dialog.getByRole('status').getByText('Link copied.', {exact: true}).waitFor({timeout});
      check(await page.evaluate(() => navigator.clipboard.readText()) === link, 'document_link_browser_copy_unbound');
    } finally {
      await page.evaluate(() => navigator.clipboard.writeText(''));
      await context.clearPermissions();
    }

    await visit('/agent-manager');
    await recipientSession();
    const [metadata, source] = await Promise.all([observe('GET', path), observe('GET', path + '/source'), visit(link)]);
    for (const response of [metadata, source]) {
      check(response.ok() && await response.request().headerValue(shareHeader) === token
        && !response.url().includes(token), 'document_link_browser_recipient_request_unbound');
    }
    const shared = await metadata.json(), sourceBody = await source.json();
    const content = page.getByRole('textbox', {name: 'Document content', exact: true});
    await content.waitFor({timeout});
    check(shared.id === documentId && shared.access === 'read' && shared.owner_subject === ownerId
      && sourceBody.id === documentId && sourceBody.revision === initial.source_revision
      && hash(sourceBody.markdown) === initial.sha256 && new URL(page.url()).hash === ''
      && await content.getAttribute('contenteditable') === 'false'
      && await button(page, 'Save').count() === 0 && await button(page, 'Share').count() === 0,
    'document_link_browser_recipient_access_unbound');
    await button(page, 'Comments').click({timeout});
    await page.getByText('You have view access to this document.', {exact: true}).waitFor({timeout});
    check(await page.getByRole('textbox', {name: 'Add a comment', exact: true}).count() === 0,
      'document_link_browser_recipient_comment_enabled');

    await ownerSession();
    await visit(route);
    await openDialog();
    await drainDialog(async () => {
      const [response] = await Promise.all([observe('DELETE', links + '/' + linkId),
        button(listed, 'Revoke').click({timeout})]);
      const request = response.request(), revoked = await response.json();
      check(response.ok() && request.postDataJSON()?.expected_revision === initial.revision + 1
        && await request.headerValue('idempotency-key') && revoked.id === documentId
        && revoked.revision === initial.revision + 2 && revoked.source_revision === initial.source_revision,
      'document_link_browser_revoke_unbound');
    });
    await button(listed, 'Revoke').waitFor({state: 'hidden', timeout});

    await visit('/agent-manager');
    await recipientSession();
    const [refused] = await Promise.all([observe('GET', path), visit(link)]);
    check(refused.status() === 404 && (await refused.json()).code === 'document_not_found'
      && await refused.request().headerValue(shareHeader) === token, 'document_link_browser_revocation_untyped');
    await page.getByRole('alert').getByRole('button', {name: 'Reload latest', exact: true}).waitFor({timeout});
    check(await content.count() === 0, 'document_link_browser_revoked_source_visible');
    for (const am of [ingress, peer]) {
      const {body} = await recipient.request(am, 'GET', path + '/source', {documentShareToken: token, allowed: [404]});
      check(body?.code === 'document_not_found', 'document_link_browser_peer_revocation_untyped');
    }
  }, async () => {
    page.off('request', observeCreation);
    try {
      await ownerSession();
      await page.evaluate(id => sessionStorage.removeItem('cma:document-share:' + id), documentId);
    } finally {
      if (attempted) {
        const active = await read(peer, '/share-links');
        check(Array.isArray(active) && active.length <= 1, 'document_link_browser_cleanup_unbound');
        if (active.length) {
          const pending = active[0];
          if (!id(linkId)) {
            check(/^[a-f0-9]{64}$/.test(token ?? '') && id(pending?.id) && pending.permission === 'read',
              'document_link_browser_cleanup_unbound');
            const {body} = await recipient.request(peer, 'GET', path, {documentShareToken: token});
            check(body.id === documentId && body.access === 'read' && body.owner_subject === ownerId,
              'document_link_browser_cleanup_unbound');
            linkId = pending.id;
          }
          check(pending.id === linkId, 'document_link_browser_cleanup_unbound');
          const current = await read(peer);
          await owner.request(peer, 'DELETE', links + '/' + linkId,
            {key: randomUUID(), body: {expected_revision: current.revision}});
        }
        for (const am of [ingress, peer]) {
          check(equal(await read(am, '/share-links'), []), 'document_link_browser_cleanup_unconfirmed');
        }
      }
    }
  }, recordFailure);
}
