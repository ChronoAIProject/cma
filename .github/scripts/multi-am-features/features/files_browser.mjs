import {randomUUID} from 'node:crypto';
import {check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;

export async function browserFileCreation({page, api, ingress, agentId, recordFailure}) {
  check(id(agentId), 'browser_files_agent_required');
  const path = `/api/v1/agents/${agentId}/files`;
  const directory = 'ci-browser-' + randomUUID();
  const filename = 'note.txt';
  const file = directory + '/' + filename;
  const peer = other(ingress);
  const read = am => api.request(am, 'GET', `${path}/content?path=${encodeURIComponent(file)}`, {direct: true});
  const folderEntry = am => findRootEntry(api, path, am, directory);
  const bothViews = () => Promise.all([folderEntry(ingress), folderEntry(peer)]);
  check((await bothViews()).every(entry => !entry), 'browser_files_initial_directory_present');
  const dialog = page.getByRole('dialog', {name: 'Files', exact: true});
  const button = name => dialog.getByRole('button', {name, exact: true});
  let folderSubmitted = false, folderAcknowledged = false;
  const create = async (folder, name, target, expected) => {
    await button(folder ? 'New folder' : 'New file').click({timeout});
    const field = dialog.getByRole('textbox', {name: folder ? 'Folder name' : 'File name', exact: true});
    await field.fill('nested/invalid', {timeout});
    await dialog.getByText('Enter a file or folder name without path separators.', {exact: true}).waitFor({timeout});
    check(await button('Create').isDisabled(), 'browser_files_invalid_name_accepted');
    await field.fill(name, {timeout});
    if (folder) folderSubmitted = true;
    const [response] = await Promise.all([
      page.waitForResponse(reply => new URL(reply.url()).pathname === path && reply.request().method() === 'POST'
        && reply.request().postDataJSON()?.path === target, {timeout}),
      button('Create').click({timeout}),
    ]);
    const body = await response.json();
    const submitted = response.request().postDataJSON();
    check(submitted.directory === folder && submitted.content_base64 === '', 'browser_files_create_intent_changed');
    if (expected === 'conflict') {
      check(response.status() === 409 && body.code === 'file_conflict', 'browser_files_existing_target_not_refused');
      await dialog.getByRole('alert').getByText('The file changed; refresh before saving.', {exact: true}).waitFor({timeout});
      await button('Cancel').click({timeout});
    } else {
      check(response.ok() && body.path === target && body.size_bytes === 0 && body.sha256 === hash(''),
        'browser_files_create_receipt_changed');
      if (folder) folderAcknowledged = true;
      await dialog.getByText(folder ? 'Folder created.' : 'File created.', {exact: true}).waitFor({timeout});
    }
  };
  await withRestoration(async () => {
    await page.waitForURL(url => url.pathname === '/agents/' + agentId, {timeout});
    await page.getByRole('button', {name: 'Show Agent details', exact: true}).click({timeout});
    await page.getByRole('button', {name: 'Browse files', exact: true}).click({timeout});
    const home = button('Sandbox root /home/');
    await home.click({timeout});
    await create(true, directory, directory);
    await button('Refresh files').click({timeout});
    await button(directory).click({timeout});
    await dialog.getByLabel('Current directory', {exact: true}).getByText('/home/' + directory, {exact: true}).waitFor({timeout});
    const folder = await folderEntry(peer);
    check(folder?.kind === 'directory' && folder.name === directory, 'browser_files_created_directory_missing_on_peer');
    await create(false, filename, file);
    const original = (await read(peer)).body;
    check(original.path === file && original.sha256 === hash('') && original.content_base64 === '',
      'browser_files_created_file_missing_on_peer');

    const replacement = 'Retain these bytes on name conflict.\n';
    await api.request(peer, 'POST', path, {direct: true, body: {path: file, expected_sha256: original.sha256,
      content_base64: Buffer.from(replacement).toString('base64')}});
    await create(false, filename, file, 'conflict');
    await button('Refresh files').click({timeout});
    await button(filename).click({timeout});
    const preview = dialog.getByRole('textbox', {name: 'Contents of ' + filename, exact: true});
    await preview.waitFor({timeout});
    check(await preview.inputValue() === replacement, 'browser_files_refresh_did_not_load_peer_content');
    const retained = (await read(peer)).body;
    check(retained.path === file && retained.sha256 === hash(replacement), 'browser_files_conflict_replaced_peer_content');

    const save = async (text, previous, status) => {
      await preview.fill(text, {timeout});
      const [response] = await Promise.all([
        page.waitForResponse(reply => new URL(reply.url()).pathname === path
          && reply.request().method() === 'POST' && reply.request().postDataJSON()?.path === file, {timeout}),
        button('Save').click({timeout}),
      ]);
      const intent = response.request().postDataJSON();
      check(response.status() === status && intent.expected_sha256 === previous
        && intent.content_base64 === Buffer.from(text).toString('base64'), 'browser_files_save_intent_changed');
      return response.json();
    };
    const confirmBytes = async text => {
      for (const am of [ingress, peer]) {
        const {body} = await read(am);
        check(body.path === file && body.sha256 === hash(text)
          && body.content_base64 === Buffer.from(text).toString('base64'), 'browser_files_saved_bytes_changed');
      }
    };
    const savedText = 'Saved from the browser.\n';
    const saved = await save(savedText, retained.sha256, 200);
    check(saved.path === file && saved.sha256 === hash(savedText), 'browser_files_save_receipt_changed');
    await dialog.getByText('File saved.', {exact: true}).waitFor({timeout});
    await confirmBytes(savedText);
    await button('Refresh files').click({timeout});
    await button(filename).click({timeout});
    await poll(() => preview.inputValue({timeout}), value => value === savedText,
      {timeout, interval: 100, code: 'browser_files_reopened_saved_bytes_changed'});

    const current = 'Peer saved a newer revision.\n';
    await api.request(peer, 'POST', path, {direct: true, body: {path: file, expected_sha256: saved.sha256,
      content_base64: Buffer.from(current).toString('base64')}});
    const staleText = 'A stale edit must remain a draft.\n';
    const refused = await save(staleText, saved.sha256, 409);
    check(refused.code === 'file_conflict', 'browser_files_stale_save_refusal_untyped');
    await dialog.getByRole('alert').getByText('The file changed; refresh before saving.', {exact: true}).waitFor({timeout});
    await confirmBytes(current);
    check(await preview.inputValue() === staleText, 'browser_files_conflict_lost_draft');

    await button('Reload file').click({timeout});
    const prompt = dialog.getByRole('alert').filter({hasText: 'Discard the unsaved changes to this file?'});
    await prompt.getByRole('button', {name: 'Cancel', exact: true}).click({timeout});
    check(await preview.inputValue() === staleText, 'browser_files_cancel_discard_lost_draft');
    await button('Reload file').click({timeout});
    await prompt.getByRole('button', {name: 'Confirm', exact: true}).click({timeout});
    await poll(() => preview.inputValue({timeout}), value => value === current,
      {timeout, interval: 100, code: 'browser_files_confirm_reload_used_stale_bytes'});
  }, async () => {
    if (!folderSubmitted) return;
    await removeFixtureDirectory({api, path, ingress, directory, acknowledged: folderAcknowledged});
  }, recordFailure);
}

async function findRootEntry(api, path, am, directory) {
  const {body} = await api.request(am, 'GET', `${path}/tree?path=`, {direct: true});
  check(body?.path === '' && Array.isArray(body.entries) && body.entries.every(entry =>
    typeof entry?.path === 'string' && entry.path.length > 0 && entry.name === entry.path
    && ['file', 'directory', 'symlink'].includes(entry.kind)), 'browser_files_root_listing_invalid');
  return body.entries.find(entry => entry.path === directory);
}

export async function removeFixtureDirectory({api, path, ingress, directory, acknowledged}) {
  const peer = other(ingress);
  const bothViews = () => Promise.all([ingress, peer].map(am => findRootEntry(api, path, am, directory)));
  if (!acknowledged) {
    const present = (await bothViews()).filter(Boolean);
    if (!present.length) return;
    check(present.every(entry => entry.kind === 'directory'), 'browser_files_cleanup_directory_replaced');
  }
  const {status, body} = await api.request(peer, 'DELETE', `${path}?path=${encodeURIComponent(directory)}&recursive=true`,
    {direct: true, allowed: [200, 404]});
  check(status === 404 ? body?.code === 'file_not_found' : body?.path === directory && body.deleted === true,
    'browser_files_cleanup_receipt_changed');
  check((await bothViews()).every(entry => !entry), 'browser_files_cleanup_still_visible');
}
