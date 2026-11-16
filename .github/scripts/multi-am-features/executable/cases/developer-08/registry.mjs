import {randomUUID} from 'node:crypto';
import {define, take, open, goto, visible, root, read, other, loseAcknowledgment, path} from './helpers.mjs';

async function cards(session) {
  await session.page.locator('.document-gallery[aria-busy="false"]').waitFor({state: 'visible'});
  return session.page.locator('.document-card').evaluateAll(nodes => nodes.map(node => new URL(node.href).pathname.split('/').pop()));
}
async function waitPage(session, receipt) {
  const expected = (await receipt.json()).items.map(item => item.id);
  await session.page.waitForFunction(ids => {
    const gallery = document.querySelector('.document-gallery');
    const actual = [...document.querySelectorAll('.document-card')].map(node => new URL(node.href).pathname.split('/').pop());
    return gallery?.getAttribute('aria-busy') === 'false' && ids.every(id => actual.includes(id));
  }, expected, {timeout: 30000});
}
async function allCards(ctx, session) {
  const first = await cards(session); let more = 0;
  while (await session.page.getByRole('button', {name: 'Load more', exact: true}).count()) {
    ctx.assert(++more <= 10, 'document_pagination_bound');
    const response = session.page.waitForResponse(value => new URL(value.url()).pathname === root
      && new URL(value.url()).searchParams.has('cursor'));
    const [receipt] = await Promise.all([response, session.page.getByRole('button', {name: 'Load more', exact: true}).click()]);
    await waitPage(session, receipt);
    if ((await receipt.json()).next === null) {
      await session.page.getByRole('button', {name: 'Load more', exact: true}).waitFor({state: 'hidden'});
    }
  }
  const result = await cards(session);
  ctx.assert(new Set(result).size === result.length && first.every(id => result.includes(id)), 'document_pagination_unique');
  return {result, pages: more + 1};
}
export const browseCase = define('browse', async ctx => {
  let fixture, session;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'documents-browse'); session = await open(ctx, fixture.actors.user.actor);
    await goto(session, '/artifacts');
    await visible(ctx, session.page.getByRole('heading', {name: 'Artifacts', exact: true}), 'artifacts_opened');
  });
  await ctx.step('step_2', async () => {
    for (const [label, allowed, expected] of [
      ['Yours', fixture.owned.map(row => row.id), fixture.owned.map(row => row.id)],
      ['Shared with you', [fixture.shared.id], [fixture.shared.id]],
      ['All', [...fixture.owned.map(row => row.id), fixture.shared.id], [...fixture.owned.map(row => row.id), fixture.shared.id]],
    ]) {
      const pending = session.page.waitForResponse(value => new URL(value.url()).pathname === root
        && !new URL(value.url()).searchParams.has('cursor'));
      const [receipt] = await Promise.all([pending, session.page.getByRole('group', {name: 'Artifact ownership'})
        .getByRole('button', {name: label, exact: true}).click()]);
      await waitPage(session, receipt);
      const listing = await allCards(ctx, session);
      ctx.assert(expected.every(id => listing.result.includes(id)), 'document_tab_expected_records');
      ctx.assert(!listing.result.includes(fixture.unrelated.id), 'document_tab_excludes_other_owner');
      const cohort = listing.result.filter(id => [...fixture.owned.map(row => row.id), fixture.shared.id].includes(id));
      ctx.assert(cohort.every(id => allowed.includes(id)), 'document_tab_scope');
    }
  });
  await ctx.step('step_3', async () => {
    const pending = session.page.waitForResponse(value => new URL(value.url()).pathname === root
      && new URL(value.url()).searchParams.get('q') === fixture.prefix);
    const [receipt] = await Promise.all([pending, session.page.getByRole('searchbox', {name: 'Search artifacts'}).fill(fixture.prefix)]);
    await waitPage(session, receipt);
    await session.page.waitForFunction(({count, prefix}) => {
      const nodes = [...document.querySelectorAll('.document-card')];
      return nodes.length === count && nodes.every(node => node.textContent.includes(prefix));
    }, {count: (await receipt.json()).items.length, prefix: fixture.prefix});
    ctx.assert((await cards(session)).length > 0, 'document_search_results');
  });
  await ctx.step('step_4', async () => {
    const listing = await allCards(ctx, session);
    ctx.assert(listing.pages >= 2 && listing.result.length === 27, 'document_real_load_more_exercised');
    await session.page.locator(`a.document-card[href="/artifacts/${fixture.document.id}"]`).click();
    await visible(ctx, session.page.getByRole('textbox', {name: 'Document title', exact: true}), 'selected_document_open');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(new URL(session.page.url()).pathname === `/artifacts/${fixture.document.id}`, 'selected_document_route_matches');
    ctx.assert(await session.page.getByRole('textbox', {name: 'Document title', exact: true}).inputValue() === fixture.document.title,
      'selected_document_title_matches');
    await session.switchAM(other(ctx.am)); await session.page.reload();
    await visible(ctx, session.page.getByRole('textbox', {name: 'Document title', exact: true}), 'selected_document_peer_read');
    const denied = await ctx.request({actor: fixture.actors.user.actor, am: other(ctx.am),
      method: 'GET', path: path(fixture.unrelated.id), allowed: [404]});
    ctx.assert(denied.status === 404, 'document_unauthorized_read_refused');
  });
});

export const createCase = define('create', async ctx => {
  let fixture, session, fault, created;
  const title = `Created once ${randomUUID()}`;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'documents-empty'); session = await open(ctx, fixture.actors.user.actor);
    await goto(session, '/artifacts');
    await session.page.getByRole('button', {name: 'New document', exact: true}).click();
    await visible(ctx, session.page.getByRole('dialog', {name: 'New document', exact: true}), 'create_document_dialog');
    fault = await loseAcknowledgment(ctx, session, 'POST', root,
      async value => { created = value; await fixture.lease.retainDocument(value.id); },
      intent => fixture.lease.retainIntent({actor: fixture.actors.user.actor, ...intent}));
  });
  await ctx.step('step_2', async () => {
    const dialog = session.page.getByRole('dialog', {name: 'New document', exact: true});
    await dialog.getByLabel('Title', {exact: true}).fill(title);
    ctx.assert(await dialog.getByLabel('Title', {exact: true}).inputValue() === title, 'document_create_title');
    ctx.assert(await dialog.getByRole('button', {name: 'Create document', exact: true}).isEnabled(), 'document_create_valid');
  });
  await ctx.step('step_3', async () => {
    await session.page.getByRole('button', {name: 'Create document', exact: true}).click();
    await visible(ctx, session.page.getByRole('button', {name: 'Retry creation', exact: true}), 'document_create_unknown_receipt');
    ctx.assert(await session.page.getByRole('dialog').getByLabel('Title', {exact: true}).isDisabled(), 'unknown_create_intent_locked');
    await session.switchAM(other(ctx.am));
    await session.page.getByRole('button', {name: 'Retry creation', exact: true}).click();
    await visible(ctx, session.page.getByRole('textbox', {name: 'Document title', exact: true}), 'document_created_editor');
    await fault.verify();
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(created?.id && new URL(session.page.url()).pathname === `/artifacts/${created.id}`, 'document_create_exact_identity');
    const seen = await read(ctx, created, fixture.actors.user.actor);
    ctx.assert(seen.owner_subject === fixture.actors.user.subject && seen.access === 'owner' && seen.title === title,
      'document_created_owned');
    const listing = (await ctx.request({actor: fixture.actors.user.actor, am: other(ctx.am), method: 'GET',
      path: root + '?scope=owned&q=' + encodeURIComponent(title)})).body;
    ctx.assert(listing.items.length === 1 && listing.items[0].id === created.id, 'document_create_replay_exactly_once');
    ctx.assert(await session.page.getByRole('textbox', {name: 'Document content', exact: true}).isEditable(), 'created_editor_editable');
  });
});
