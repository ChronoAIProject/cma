import {randomUUID} from 'node:crypto';
import {check, hash, id, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;

export async function browserFileCreation({page, api, ingress, agentId, recordFailure}) {
  check(id(agentId), 'browser_files_agent_required');
  const path = `/api/v1/agents/${agentId}/files`;
  const directory = 'ci-browser-' + randomUUID();
  const filename = 'note.txt';
  const file = directory + '/' + filename;
  const peer = other(ingress);
  const list = (am, parent = '') => api.request(am, 'GET', `${path}/tree?path=${encodeURIComponent(parent)}`, {direct: true});
  const read = am => api.request(am, 'GET', `${path}/content?path=${encodeURIComponent(file)}`, {direct: true});
  const folderEntry = response => response.body?.entries?.find(entry => entry.path === directory);
  const initial = await list(peer);
  check(Array.isArray(initial.body?.entries) && !folderEntry(initial), 'browser_files_initial_directory_present');
  const dialog = page.getByRole('dialog', {name: 'Files', exact: true});
  const button = name => dialog.getByRole('button', {name, exact: true});
  let folderSubmitted = false;
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
    const folder = folderEntry(await list(peer));
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
  }, async () => {
    if (!folderSubmitted) return;
    const observed = await list(peer);
    if (!folderEntry(observed)) return;
    const {body} = await api.request(peer, 'DELETE', `${path}?path=${encodeURIComponent(directory)}&recursive=true`, {direct: true});
    check(body?.path === directory && body.deleted === true, 'browser_files_cleanup_receipt_changed');
    for (const am of [ingress, peer]) {
      const after = await list(am);
      check(Array.isArray(after.body?.entries) && !folderEntry(after), 'browser_files_cleanup_still_visible');
    }
  }, recordFailure);
}
