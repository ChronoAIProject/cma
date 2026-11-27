import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;
const endpoint = '/api/v1/documents';
const outlineHeading = 'Synthetic acceptance result';
const code = 'print("Fixture code")';
const previewMarkdown = ['# Document overview', ...Array.from({length: 30}, (_, index) => `Fixture paragraph ${index + 1}`),
  '## ' + outlineHeading, '~~~python\n' + code + '\n~~~', '~~~mermaid\nflowchart LR\n  A --> B\n~~~'].join('\n\n') + '\n';

async function readPreview(page) {
  const content = page.getByRole('textbox', {name: 'Document content', exact: true});
  await content.waitFor({timeout});
  check(await content.getAttribute('contenteditable', {timeout}) === 'false', 'document_preview_editable');
  const heading = content.getByRole('heading', {name: outlineHeading, exact: true});
  const insideViewport = () => heading.evaluate(node => {
    const {top, bottom} = node.getBoundingClientRect();
    return top >= 0 && bottom <= window.innerHeight;
  }, undefined, {timeout});
  check(!await insideViewport(), 'document_preview_heading_already_in_view');
  await page.getByRole('button', {name: 'On this page', exact: true}).click({timeout});
  const outline = page.getByRole('navigation', {name: 'On this page', exact: true});
  await outline.getByRole('button', {name: outlineHeading, exact: true}).click({timeout});
  await poll(insideViewport, Boolean, {timeout, interval: 100, code: 'document_preview_outline_did_not_scroll'});
  await content.locator('pre code').getByText(code, {exact: true}).waitFor({timeout});
  const diagram = content.getByRole('img', {name: 'Mermaid diagram', exact: true});
  await diagram.waitFor({timeout});
  check(await diagram.evaluate(image => image.complete && image.naturalWidth > 0),
    'document_preview_diagram_did_not_render');
}

export async function documentBrowse({page, ingress, user, admin, ownerId, recordFailure}) {
  const peer = other(ingress);
  const prefix = 'Browse ' + randomUUID();
  const ownedTitle = prefix + ' owned';
  const sharedTitle = prefix + ' shared';
  const foreignTitle = prefix + ' private';
  let shared;
  const create = async (actor, am, title, markdown = '# ' + title + '\n') => {
    const {body: document} = await actor.request(am, 'POST', endpoint, {key: randomUUID(),
      body: {title, markdown}, allowed: [201]});
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
    const owned = await create(user, ingress, ownedTitle, previewMarkdown);
    shared = await create(admin, peer, sharedTitle, previewMarkdown);
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
      await readPreview(page);
      check(await page.getByRole('button', {name: 'Save', exact: true}).count() === 0,
        'document_preview_recipient_can_save');
      await page.goto(`/artifacts/${owned.id}`, {waitUntil: 'domcontentloaded', timeout});
      const mode = page.getByRole('group', {name: 'Document mode', exact: true});
      await mode.getByRole('button', {name: 'Preview', exact: true}).click({timeout});
      check(await title.inputValue({timeout}) === ownedTitle && await title.isDisabled(),
        'document_preview_owner_mode_not_read_only');
      await readPreview(page);
      await mode.getByRole('button', {name: 'Edit', exact: true}).click({timeout});
      check(await title.isEnabled(), 'document_preview_edit_not_restored');
      const {body: after} = await user.request(peer, 'GET', `${endpoint}/${owned.id}`);
      check(after.revision === owned.revision && after.source_revision === owned.source_revision,
        'document_preview_changed_saved_source');
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
