import {randomUUID} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import {check, hash, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;
const oversizeBytes = 32 * 1024 * 1024 + 1;

export async function browserFileRoundtrip({page, api, outsider, ingress, agentId, seedOversizeFile, recordFailure}) {
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
  let folderSubmitted = false, folderAcknowledged = false, folderDeleted = false;
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

    const hiddenName = '.fixture-note';
    const hiddenPath = directory + '/' + hiddenName;
    const hiddenText = 'Hidden fixture entry.\n';
    const {body: hiddenReceipt} = await api.request(peer, 'POST', path, {direct: true,
      body: {path: hiddenPath, content_base64: Buffer.from(hiddenText).toString('base64')}});
    check(hiddenReceipt.path === hiddenPath && hiddenReceipt.sha256 === hash(hiddenText),
      'browser_files_hidden_fixture_unbound');
    const [listing] = await Promise.all([
      page.waitForResponse(reply => {
        const url = new URL(reply.url());
        return reply.request().method() === 'GET' && url.pathname === path + '/tree'
          && url.searchParams.get('path') === directory;
      }, {timeout}),
      button('Refresh files').click({timeout}),
    ]);
    const tree = await listing.json();
    check(listing.status() === 200 && tree.path === directory && Array.isArray(tree.entries)
      && tree.entries.some(entry => entry.path === hiddenPath && entry.name === hiddenName
        && entry.hidden === true && entry.kind === 'file'), 'browser_files_refreshed_hidden_entry_unbound');
    await button(filename).waitFor({timeout});
    check(await button(hiddenName).count() === 0, 'browser_files_hidden_entry_visible_by_default');
    const showHidden = dialog.getByRole('checkbox', {name: 'Show hidden', exact: true});
    await showHidden.check({timeout});
    await button(hiddenName).waitFor({timeout});
    check(await button(hiddenName).getAttribute('title') === '/home/' + hiddenPath,
      'browser_files_hidden_entry_path_changed');
    await button(hiddenName).click({timeout});
    await dialog.getByLabel('Selected file', {exact: true}).getByText('/home/' + hiddenPath, {exact: true}).waitFor({timeout});
    const hiddenPreview = dialog.getByRole('textbox', {name: 'Contents of ' + hiddenName, exact: true});
    await poll(() => hiddenPreview.inputValue({timeout}), value => value === hiddenText,
      {timeout, interval: 100, code: 'browser_file_tree_refreshed_bytes_changed'});
    const removed = await api.request(peer, 'DELETE', `${path}?path=${encodeURIComponent(hiddenPath)}&recursive=false`,
      {direct: true});
    check(removed.body?.path === hiddenPath && removed.body.deleted === true,
      'browser_files_download_fixture_delete_unconfirmed');
    await checkFileDownload({page, dialog, path, file: hiddenPath, refusal: {
      status: 404, code: 'file_not_found', detail: 'File or directory does not exist.',
    }});
    await showHidden.uncheck({timeout});
    await button(hiddenName).waitFor({state: 'hidden', timeout});
    const currentDirectory = dialog.getByLabel('Current directory', {exact: true});
    const folderButton = button(directory);
    await folderButton.click({timeout});
    check(await folderButton.getAttribute('aria-expanded') === 'false',
      'browser_files_folder_did_not_collapse');
    await home.click({timeout});
    await currentDirectory.getByText('/home/', {exact: true}).waitFor({timeout});
    await folderButton.click({timeout});
    await currentDirectory.getByText('/home/' + directory, {exact: true}).waitFor({timeout});
    await button(filename).waitFor({timeout});
    for (const am of [ingress, peer]) {
      for (const rejected of ['cma/.cma', '../fixture-outside-home', '/']) {
        const {status, body} = await api.request(am, 'GET', path + '/tree?path=' + encodeURIComponent(rejected),
          {direct: true, allowed: [200, 400]});
        check(status === 400 && body?.code === 'invalid_file_request' && !Object.hasOwn(body, 'entries'),
          'browser_files_unscoped_listing_not_refused');
      }
    }

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
    await checkFileDownload({page, dialog, path, file, expected: Buffer.from(current)});
    for (const am of [ingress, peer]) {
      await outsider.rawRequest(am, 'GET', `${path}/download?path=${encodeURIComponent(file)}`,
        {allowed: [403, 404], discardSuccessfulReply: true});
    }

    const oversize = directory + '/oversize.txt';
    check((await seedOversizeFile(directory)).size_bytes === oversizeBytes, 'browser_files_download_oversize_seed_unbound');
    const {body: oversizeListing} = await api.request(peer, 'GET', `${path}/tree?path=${encodeURIComponent(directory)}`,
      {direct: true});
    check(oversizeListing?.path === directory && oversizeListing.entries?.some(entry =>
      entry.path === oversize && entry.kind === 'file' && entry.size_bytes === oversizeBytes),
    'browser_files_download_oversize_missing_on_peer');
    await button('Refresh files').click({timeout});
    await button('oversize.txt').click({timeout});
    await dialog.getByText('Preview unavailable. Download this file to view its contents.', {exact: true}).waitFor({timeout});
    await checkFileDownload({page, dialog, path, file: oversize, refusal: {
      status: 413, code: 'file_too_large', detail: 'File exceeds the selected size limit.',
    }});
    await button(filename).click({timeout});
    await preview.waitFor({timeout});
    await deleteFiles({page, dialog, api, ingress, path, directory, file, recordFailure});
    for (const am of [ingress, peer]) {
      const {body} = await api.rawRequest(am, 'GET', `${path}/download?path=${encodeURIComponent(oversize)}`,
        {allowed: [404], discardSuccessfulReply: true});
      check(body?.code === 'file_not_found', 'browser_files_download_oversize_cleanup_unconfirmed');
    }
    folderDeleted = true;
  }, async () => {
    if (!folderSubmitted || folderDeleted) return;
    await removeFixtureDirectory({api, path, ingress, directory, acknowledged: folderAcknowledged});
  }, recordFailure);
}

async function checkFileDownload({page, dialog, path, file, expected, refusal}) {
  const button = dialog.getByRole('button', {name: 'Download file', exact: true});
  const responseForFile = () => page.waitForResponse(response => {
    const url = new URL(response.url());
    return response.request().method() === 'GET' && url.origin === new URL(page.url()).origin
      && url.pathname === path + '/download' && url.searchParams.get('path') === file;
  }, {timeout});
  if (expected) {
    const [response, download] = await Promise.all([
      responseForFile(), page.waitForEvent('download', {timeout}), button.click({timeout}),
    ]);
    try {
      check(response.status() === 200 && response.headers()['content-type'] === 'application/octet-stream',
        'browser_files_download_response_invalid');
      check(download.suggestedFilename() === file.split('/').at(-1)
        && (await readFile(await download.path())).equals(expected), 'browser_files_download_bytes_changed');
    } finally {
      await download.delete();
    }
    return;
  }
  const unexpected = [];
  const capture = download => unexpected.push(download);
  page.on('download', capture);
  try {
    const [response] = await Promise.all([responseForFile(), button.click({timeout})]);
    check(response.status() === refusal.status && (await response.json()).code === refusal.code,
      'browser_files_download_refusal_untyped');
    await dialog.getByRole('alert').getByText(refusal.detail, {exact: true}).waitFor({timeout});
    check(unexpected.length === 0, 'browser_files_refused_download_produced_file');
  } finally {
    page.off('download', capture);
    await Promise.all(unexpected.map(download => download.delete()));
  }
}

async function deleteFiles({page, dialog, api, ingress, path, directory, file, recordFailure}) {
  const peer = other(ingress);
  const sibling = directory + '/retained.txt';
  const write = {path: sibling, content_base64: Buffer.from('Retain until recursive deletion.').toString('base64')};
  const {body: siblingReceipt} = await api.request(peer, 'POST', path, {direct: true, body: write});
  check(siblingReceipt.path === sibling, 'browser_files_delete_sibling_unbound');
  const read = (am, target, allowed) => api.request(am, 'GET', `${path}/content?path=${encodeURIComponent(target)}`,
    {direct: true, allowed});
  const button = name => dialog.getByRole('button', {name, exact: true});
  const confirmation = (target, recursive) => dialog.getByRole('alert').filter({
    hasText: `Delete /home/${target}${recursive ? ' and everything inside it' : ''}? This cannot be undone.`,
  });
  const confirmDelete = async (name, target, recursive) => {
    await button(name).click({timeout});
    const prompt = confirmation(target, recursive);
    await prompt.waitFor({timeout});
    const [response] = await Promise.all([
      page.waitForResponse(reply => {
        const url = new URL(reply.url());
        return url.pathname === path && reply.request().method() === 'DELETE'
          && url.searchParams.get('path') === target && url.searchParams.get('recursive') === String(recursive);
      }, {timeout}),
      prompt.getByRole('button', {name: 'Confirm', exact: true}).click({timeout}),
    ]);
    const body = await response.json();
    check(response.ok() && body.path === target && body.deleted === true, 'browser_files_delete_receipt_changed');
    await dialog.getByText('Deleted.', {exact: true}).waitFor({timeout});
  };
  const neighbor = directory + '-neighbor.txt';
  const neighborText = 'Outside the confirmed recursive deletion.';
  const neighborBody = Buffer.from(neighborText).toString('base64');
  const absent = async am => {
    const {status, body} = await read(am, neighbor, [200, 404]);
    return status === 404 && body?.code === 'file_not_found';
  };
  const exactNeighbor = async am => {
    const {status, body} = await read(am, neighbor, [200, 404]);
    return status === 200 && body?.path === neighbor && body.sha256 === hash(neighborText)
      && body.content_base64 === neighborBody;
  };
  check((await Promise.all([ingress, peer].map(absent))).every(Boolean), 'browser_files_delete_neighbor_present');
  await withRestoration(async () => {
    const {body} = await api.request(peer, 'POST', path, {direct: true,
      body: {path: neighbor, content_base64: neighborBody}});
    check(body?.path === neighbor && body.sha256 === hash(neighborText), 'browser_files_delete_neighbor_receipt');
    await button('Delete file').click({timeout});
    await confirmation(file, false).getByRole('button', {name: 'Cancel', exact: true}).click({timeout});
    check((await read(peer, file, [200])).body.path === file, 'browser_files_cancel_deleted_target');
    await confirmDelete('Delete file', file, false);
    const filename = file.split('/').at(-1);
    await button(filename).waitFor({state: 'hidden', timeout});
    for (const am of [ingress, peer]) {
      check((await read(am, file, [404])).body?.code === 'file_not_found', 'browser_files_deleted_file_still_visible');
      const {body} = await read(am, sibling, [200]);
      check(body.path === sibling && body.sha256 === siblingReceipt.sha256,
        'browser_files_delete_changed_unselected_sibling');
    }

    check((await Promise.all([ingress, peer].map(exactNeighbor))).every(Boolean),
      'browser_files_delete_neighbor_unbound');
    await confirmDelete('Delete folder', directory, true);
    await button('Refresh files').click({timeout});
    await button(directory).waitFor({state: 'hidden', timeout});
    for (const am of [ingress, peer]) {
      check(!await findRootEntry(api, path, am, directory), 'browser_files_deleted_directory_still_visible');
      check((await read(am, sibling, [404])).body?.code === 'file_not_found', 'browser_files_recursive_delete_left_child');
    }
    check((await Promise.all([ingress, peer].map(exactNeighbor))).every(Boolean),
      'browser_files_recursive_delete_changed_neighbor');
  }, async () => {
    const {status, body} = await api.request(peer, 'DELETE', `${path}?path=${encodeURIComponent(neighbor)}&recursive=false`,
      {direct: true, allowed: [200, 404]});
    check(status === 404 ? body?.code === 'file_not_found' : body?.path === neighbor && body.deleted === true,
      'browser_files_delete_neighbor_cleanup_changed');
    check((await Promise.all([ingress, peer].map(absent))).every(Boolean),
      'browser_files_delete_neighbor_cleanup_unconfirmed');
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
