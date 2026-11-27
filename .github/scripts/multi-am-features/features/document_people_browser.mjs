import {randomUUID} from 'node:crypto';
import {check, id, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;

export async function documentPeople({page, ingress, owner, recipient, documentId, recipientId, recordFailure}) {
  check(id(documentId) && id(recipientId), 'document_people_fixture_invalid');
  const peer = other(ingress);
  const path = '/api/v1/documents/' + documentId;
  const shares = path + '/shares';
  const grant = shares + '/' + recipientId;
  const readOwner = async () => (await owner.request(peer, 'GET', path)).body;
  const readShares = async () => (await owner.request(peer, 'GET', shares)).body;
  const readDenied = async () => {
    for (const am of [ingress, peer]) {
      await recipient.request(am, 'GET', path, {allowed: [404]});
      await recipient.request(am, 'GET', path + '/source', {allowed: [404]});
    }
  };
  const observedMutation = (method, revision, action, permission) => Promise.all([
    page.waitForResponse(response => response.request().method() === method
      && new URL(response.url()).pathname === grant, {timeout}),
    action(),
  ]).then(async ([response]) => {
    const request = response.request().postDataJSON();
    const body = await response.json();
    check(response.status() === 200 && request.expected_revision === revision
      && request.permission === permission && body.id === documentId && body.revision === revision + 1,
    'document_people_mutation_unbound');
    return body;
  });
  await page.goto('/artifacts/' + documentId, {waitUntil: 'domcontentloaded', timeout});
  await page.getByRole('button', {name: 'Share', exact: true}).click({timeout});
  const dialog = page.getByRole('dialog', {name: /^Share /});
  const people = dialog.getByRole('list', {name: 'People with access', exact: true});
  const row = people.getByRole('listitem').filter({has: page.getByText(recipientId, {exact: true})});
  const search = dialog.getByRole('textbox', {name: 'Find share recipients', exact: true});
  const add = dialog.getByRole('button', {name: 'Add person', exact: true});
  await withRestoration(async () => {
    check((await readShares()).length === 0, 'document_people_fixture_not_private');
    await readDenied();
    let current = await readOwner();
    for (const [permission, label] of [['read', 'Can view'], ['comment', 'Can comment']]) {
      if (permission === 'comment') {
        await dialog.getByRole('button', {name: 'Recipient permission', exact: true}).click({timeout});
        await page.getByRole('menuitemradio', {name: label, exact: true}).click({timeout});
      }
      await search.fill(recipientId, {timeout});
      const match = dialog.getByRole('list', {name: 'Matching people', exact: true})
        .getByRole('listitem').filter({has: page.getByText(recipientId, {exact: true})});
      await match.getByRole('button', {name: /^Select /}).click({timeout});
      current = await observedMutation('PUT', current.revision, () => add.click({timeout}), permission);
      await row.getByText(label, {exact: true}).waitFor({timeout});
      for (const am of [ingress, peer]) {
        const {body} = await recipient.request(am, 'GET', path);
        check(body.id === documentId && body.revision === current.revision && body.access === permission,
          'document_people_recipient_access_unbound');
      }
      current = await observedMutation('DELETE', current.revision,
        () => row.getByRole('button', {name: 'Remove', exact: true}).click({timeout}));
      await row.waitFor({state: 'hidden', timeout});
      await readDenied();
    }
    await dialog.getByRole('button', {name: 'Done', exact: true}).click({timeout});
  }, async () => {
    const grants = await readShares();
    if (grants.some(item => item.subject === recipientId)) {
      const current = await readOwner();
      await owner.request(peer, 'DELETE', grant, {key: randomUUID(), body: {expected_revision: current.revision}});
    }
    await readDenied();
  }, recordFailure);
}
