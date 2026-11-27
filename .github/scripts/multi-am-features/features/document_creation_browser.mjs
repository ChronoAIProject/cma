import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, id, other} from '../cma/core.mjs';

const timeout = 15000;
const registry = '/artifacts';
const documents = '/api/v1/documents';

export async function createDocumentAfterLostReply({page, ingress, user, ownerId}) {
  const title = 'Fixture document ' + randomUUID();
  const isCreate = request => request.method() === 'POST' && new URL(request.url()).pathname === documents;
  const creation = async request => ({key: await request.headerValue('idempotency-key'), body: request.postDataJSON()});
  const button = name => page.getByRole('button', {name, exact: true});
  const endpoint = url => url.pathname === documents;
  const attempts = [];
  let original;
  let failure;
  const loseReply = async route => {
    if (!isCreate(route.request())) return route.fallback();
    try {
      attempts.push(await creation(route.request()));
      check(attempts.length === 1 && attempts[0].key && attempts[0].body?.title === title,
        'document_browser_original_request_unbound');
      const response = await route.fetch({timeout, maxRedirects: 0, maxRetries: 0});
      check(response.status() === 201, 'document_browser_original_create_refused');
      original = await response.json();
      check(id(original?.id) && original.title === title && original.owner_subject === ownerId
        && original.access === 'owner', 'document_browser_original_create_unbound');
    } catch (error) {
      failure = error;
    } finally {
      await route.abort('failed');
    }
  };

  await page.goto(registry, {waitUntil: 'domcontentloaded', timeout});
  await page.route(endpoint, loseReply);
  const read = async am => (await user.request(am, 'GET', `${documents}/${original.id}`)).body;
  try {
    await button('New document').click({timeout});
    await page.getByRole('textbox', {name: 'Title', exact: true}).fill(title, {timeout});
    await button('Create document').click({timeout});
    await page.getByText('Creation not confirmed', {exact: true}).waitFor({timeout});
    if (failure) throw failure;
    check(original && equal(await read(other(ingress)), original), 'document_browser_commit_missing_on_peer');
  } finally {
    await page.unroute(endpoint, loseReply);
  }

  await page.reload({waitUntil: 'domcontentloaded', timeout});
  await button('Resume creation').click({timeout});
  const retainedTitle = page.getByRole('textbox', {name: 'Title', exact: true});
  check(await retainedTitle.inputValue({timeout}) === title && await retainedTitle.isDisabled(),
    'document_browser_original_title_not_retained');
  const [retry, response] = await Promise.all([
    page.waitForRequest(isCreate, {timeout}),
    page.waitForResponse(response => isCreate(response.request()), {timeout}),
    button('Retry creation').click({timeout}),
  ]);
  check(equal(await creation(retry), attempts[0]), 'document_browser_retry_changed_intent');
  check(response.status() === 201 && equal(await response.json(), original), 'document_browser_retry_changed_receipt');
  await page.waitForURL(url => url.pathname === `${registry}/${original.id}`, {timeout});
  const editorTitle = page.getByRole('textbox', {name: 'Document title', exact: true});
  check(await editorTitle.inputValue({timeout}) === title, 'document_browser_editor_title_changed');
  for (const am of [ingress, other(ingress)]) {
    const list = (await user.request(am, 'GET', `${documents}?scope=owned&q=${encodeURIComponent(title)}`)).body;
    const matches = list.items?.filter(item => item.title === title);
    check(matches?.length === 1 && matches[0].id === original.id && equal(await read(am), original),
      'document_browser_retry_duplicated_or_changed_commit');
  }
}
