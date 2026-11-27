import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, id, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;
const endpoint = '/api/v1/documents';

export async function documentBrowse({page, ingress, user, admin, ownerId, recordFailure}) {
  const peer = other(ingress);
  const prefix = 'Browse ' + randomUUID();
  const ownedTitle = prefix + ' owned';
  const sharedTitle = prefix + ' shared';
  const foreignTitle = prefix + ' private';
  let shared;
  const create = async (actor, am, title) => {
    const {body: document} = await actor.request(am, 'POST', endpoint, {key: randomUUID(),
      body: {title, markdown: '# ' + title + '\n'}, allowed: [201]});
    check(id(document?.id) && document.title === title && document.access === 'owner', 'document_browse_seed_unbound');
    return document;
  };
  const onlyRegistry = url => url.pathname === endpoint && url.searchParams.get('q') === prefix;
  const onePerPage = route => {
    if (route.request().method() !== 'GET') return route.fallback();
    const url = new URL(route.request().url());
    url.searchParams.set('limit', '1');
    return route.continue({url: url.href});
  };
  await withRestoration(async () => {
    const owned = await create(user, ingress, ownedTitle);
    shared = await create(admin, peer, sharedTitle);
    const foreign = await create(admin, peer, foreignTitle);
    const sharePath = `${endpoint}/${shared.id}/shares/${encodeURIComponent(ownerId)}`;
    const {body: updated} = await admin.request(peer, 'PUT', sharePath, {key: randomUUID(),
      body: {permission: 'read', expected_revision: shared.revision}});
    check(updated?.id === shared.id && updated.revision === shared.revision + 1,
      'document_browse_share_unbound');
    await user.request(peer, 'GET', `${endpoint}/${foreign.id}`, {allowed: [404]});

    const buttons = page.getByRole('group', {name: 'Artifact ownership', exact: true});
    const gallery = page.locator('.document-gallery');
    const link = document => gallery.locator(`a[href="/artifacts/${document.id}"]`);
    const assertVisible = async documents => {
      for (const document of documents) await link(document).waitFor({timeout});
      const shown = await gallery.getByRole('link').evaluateAll(links => links.map(link => link.getAttribute('href')));
      check(equal(shown.sort(), documents.map(document => `/artifacts/${document.id}`).sort()),
        'document_browse_visible_scope_wrong');
    };
    const observe = async (scope, action, cursor = null) => {
      const [response] = await Promise.all([
        page.waitForResponse(response => {
          const url = new URL(response.url());
          return onlyRegistry(url) && response.request().method() === 'GET'
            && url.searchParams.get('scope') === scope && url.searchParams.get('cursor') === cursor;
        }, {timeout}), action(),
      ]);
      check(response.ok(), 'document_browse_page_refused');
      const body = await response.json();
      check(body.items?.length === 1, 'document_browse_page_size_wrong');
      return body;
    };

    await page.route(onlyRegistry, onePerPage);
    try {
      await page.goto('/artifacts', {waitUntil: 'domcontentloaded', timeout});
      const first = await observe('all', () => page.getByRole('searchbox', {name: 'Search artifacts'}).fill(prefix, {timeout}));
      check([owned.id, shared.id].includes(first.items[0].id) && typeof first.next === 'string' && first.next.length > 0,
        'document_browse_first_page_unbound');
      await assertVisible(first.items);
      const second = await observe('all', () => page.getByRole('button', {name: 'Load more', exact: true}).click({timeout}), first.next);
      check(second.next == null && equal([first.items[0].id, second.items[0].id].sort(), [owned.id, shared.id].sort()),
        'document_browse_cursor_changed_scope');
      await assertVisible([owned, shared]);
      for (const [scope, label, expected, permission] of [['owned', 'Yours', owned, 'owner'], ['shared', 'Shared with you', shared, 'read']]) {
        const result = await observe(scope, () => buttons.getByRole('button', {name: label, exact: true}).click({timeout}));
        check(result.next == null && result.items[0].id === expected.id && result.items[0].access === permission,
          'document_browse_tab_scope_wrong');
        await assertVisible([expected]);
      }
      await link(shared).click({timeout});
      await page.waitForURL(url => url.pathname === `/artifacts/${shared.id}`, {timeout});
      const title = page.getByRole('textbox', {name: 'Document title', exact: true});
      check(await title.inputValue({timeout}) === sharedTitle && await title.isDisabled(),
        'document_browse_shared_open_not_read_only');
    } finally {
      await page.unroute(onlyRegistry, onePerPage);
    }
  }, async () => {
    // The disposable database owns the created Documents. Restore access using
    // the grant API, which is available with document deletion disabled.
    if (!shared) return;
    const path = `${endpoint}/${shared.id}`;
    const {body: shares} = await admin.request(peer, 'GET', path + '/shares');
    check(Array.isArray(shares), 'document_browse_grant_readback_failed');
    if (shares.some(row => row.subject === ownerId)) {
      const {body: current} = await admin.request(peer, 'GET', path);
      const {body: receipt} = await admin.request(peer, 'DELETE', path + '/shares/' + encodeURIComponent(ownerId),
        {key: randomUUID(), body: {expected_revision: current.revision}});
      check(receipt.id === shared.id && receipt.revision === current.revision + 1,
        'document_browse_cleanup_unbound');
    }
    for (const am of ['a', 'b']) await user.request(am, 'GET', path, {allowed: [404]});
  }, recordFailure);
}
