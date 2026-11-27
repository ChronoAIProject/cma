import {randomUUID} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, other} from '../cma/core.mjs';

const timeout = 15000;

export async function documentEditAndConflict({page, ingress, user, document}) {
  const path = `/api/v1/documents/${document.id}`;
  const peer = other(ingress);
  const title = page.getByRole('textbox', {name: 'Document title', exact: true});
  const content = page.getByRole('textbox', {name: 'Document content', exact: true});
  const button = name => page.getByRole('button', {name, exact: true});
  const status = page.locator('.document-page__save-status');
  const writeDraft = async (name, text) => {
    await title.fill(name, {timeout});
    await content.fill(text, {timeout});
    await status.getByText('Unsaved changes', {exact: true}).waitFor({timeout});
  };
  const save = async (revision, name, marker, expectedStatus) => {
    const [response] = await Promise.all([
      page.waitForResponse(value => value.request().method() === 'PATCH'
        && new URL(value.url()).pathname === path, {timeout}),
      button('Save').click({timeout}),
    ]);
    const body = response.request().postDataJSON();
    check(body.expected_revision === revision && body.title === name && body.markdown.includes(marker)
      && response.status() === expectedStatus, 'document_browser_save_unbound');
    return {body: await response.json(), markdown: body.markdown};
  };
  const assertSource = async (am, expected, marker) => {
    const {body: seen} = await user.request(am, 'GET', path);
    const {body: source} = await user.request(am, 'GET', `${path}/source`);
    check(equal(seen, expected) && source.id === expected.id && source.revision === expected.source_revision
      && source.markdown.includes(marker), 'document_browser_source_changed');
  };

  const savedTitle = document.title + ' edited';
  const savedContent = 'Saved through the browser';
  await writeDraft(savedTitle, savedContent);
  const {body: saved} = await save(document.revision, savedTitle, savedContent, 200);
  check(saved.id === document.id && saved.title === savedTitle && saved.revision === document.revision + 1
    && saved.source_revision > document.source_revision, 'document_browser_save_revision_changed');
  await status.getByText('Saved', {exact: true}).waitFor({timeout});
  await assertSource(peer, saved, savedContent);
  await page.reload({waitUntil: 'domcontentloaded', timeout});
  await content.filter({hasText: savedContent}).waitFor({timeout});
  check(await title.inputValue({timeout}) === savedTitle, 'document_browser_save_did_not_reload');

  const staleTitle = document.title + ' draft';
  const staleContent = 'Draft retained after conflict';
  await writeDraft(staleTitle, staleContent);
  const peerTitle = document.title + ' peer revision';
  const peerContent = 'Newer peer source';
  const {body: latest} = await user.request(peer, 'PATCH', path, {key: randomUUID(),
    body: {expected_revision: saved.revision, title: peerTitle, markdown: peerContent + '\n'}});
  check(latest.id === document.id && latest.title === peerTitle && latest.revision === saved.revision + 1
    && latest.source_revision > saved.source_revision, 'document_browser_peer_revision_unbound');
  const stale = await save(saved.revision, staleTitle, staleContent, 409);
  await status.getByText('Version changed', {exact: true}).waitFor({timeout});
  await content.filter({hasText: staleContent}).waitFor({timeout});
  check(await title.inputValue({timeout}) === staleTitle, 'document_browser_conflict_lost_draft');

  const [download] = await Promise.all([
    page.waitForEvent('download', {timeout}), button('Download draft').click({timeout}),
  ]);
  try {
    const markdown = await readFile(await download.path(), 'utf8');
    check(markdown === stale.markdown, 'document_browser_downloaded_wrong_draft');
  } finally {
    await download.delete();
  }
  const confirm = page.getByRole('dialog', {name: 'Load the latest document?', exact: true});
  await button('Reload latest').click({timeout});
  await confirm.getByRole('button', {name: 'Close', exact: true}).click({timeout});
  check(await title.inputValue({timeout}) === staleTitle, 'document_browser_cancel_discard_lost_draft');
  await button('Reload latest').click({timeout});
  await confirm.getByRole('button', {name: 'Discard draft and reload', exact: true}).click({timeout});
  await content.filter({hasText: peerContent}).waitFor({timeout});
  check(await title.inputValue({timeout}) === peerTitle, 'document_browser_reload_used_wrong_revision');
  for (const am of [ingress, peer]) await assertSource(am, latest, peerContent);
}
