import {readFile} from 'node:fs/promises';
import {define, setup, open, button, visible, raw, read, write, other, hash, observedPeer} from './helpers.mjs';

async function create(ctx, directory) {
  const f = await setup(ctx), {page} = f, name = directory ? 'created-folder' : 'created.txt';
  await ctx.step('step_1', async () => {await open(ctx, f); await visible(ctx, button(page, f.root), 'directory_selected');});
  await ctx.step('step_2', async () => {
    await page.getByRole('button', {name: directory ? 'New folder' : 'New file', exact: true}).click();
    const input = page.getByLabel(directory ? 'Folder name' : 'File name', {exact: true});
    await input.fill('../escape'); await page.getByRole('button', {name: 'Create', exact: true}).click();
    await visible(ctx, page.getByText('Enter a file or folder name without path separators.', {exact: true}), 'invalid_name_refused');
    await input.fill(name); await page.getByRole('button', {name: 'Create', exact: true}).click();
    await visible(ctx, page.getByText(directory ? 'Folder created.' : 'File created.', {exact: true}), 'create_acknowledged');
  });
  await ctx.step('step_3', async () => {
    await page.getByRole('button', {name: 'Refresh files', exact: true}).click();
    await button(page, name).click();
    if (directory) ctx.assert(await button(page, name).getAttribute('aria-expanded') === 'true', 'folder_opened');
    else ctx.assert(await page.getByRole('textbox', {name: 'Contents of ' + name, exact: true}).inputValue() === '', 'empty_file_opened');
  });
  await ctx.step('expected_result', async () => {
    const tree = await raw(ctx, f, 'GET', '/tree', f.root, {am: await observedPeer(ctx, f)});
    ctx.assert(tree.body.entries.filter(row => row.name === name && row.path === f.root + '/' + name
      && row.kind === (directory ? 'directory' : 'file')).length === 1, 'exact_entry_created');
    // Return to the original parent before repeating the same target through UI.
    await page.reload(); await open(ctx, f);
    await page.getByRole('button', {name: directory ? 'New folder' : 'New file', exact: true}).click();
    await page.getByLabel(directory ? 'Folder name' : 'File name', {exact: true}).fill(name);
    const response = page.waitForResponse(r => r.request().method() === 'POST'
      && /\/files$/.test(new URL(r.url()).pathname));
    await page.getByRole('button', {name: 'Create', exact: true}).click();
    ctx.assert((await response).status() === 409, 'existing_target_conflict');
  });
}

export const createFile = define('create-file', ctx => create(ctx, false));
export const createDirectory = define('create-directory', ctx => create(ctx, true));
export const browse = define('browse', async ctx => {
  const f = await setup(ctx), {page} = f;
  await write(ctx, f, f.root + '/visible.txt', 'visible'); await write(ctx, f, f.root + '/.hidden.txt', 'hidden');
  await ctx.step('step_1', async () => {await open(ctx, f); await visible(ctx, button(page, 'visible.txt'), 'files_visible');});
  await ctx.step('step_2', async () => {
    await visible(ctx, page.getByTitle('/home/' + f.root, {exact: true}).last(), 'breadcrumb_scoped');
    ctx.assert(await button(page, f.root).getAttribute('aria-expanded') === 'true', 'directory_expanded');
  });
  await ctx.step('step_3', async () => {
    ctx.assert(await button(page, '.hidden.txt').count() === 0, 'hidden_initially_absent');
    await page.getByRole('checkbox', {name: 'Show hidden', exact: true}).check();
    await visible(ctx, button(page, '.hidden.txt'), 'hidden_visible');
    await page.getByRole('checkbox', {name: 'Show hidden', exact: true}).uncheck();
    await button(page, '.hidden.txt').waitFor({state: 'hidden'}); ctx.assert(true, 'hidden_toggle');
  });
  await ctx.step('step_4', async () => {
    await write(ctx, f, f.root + '/new.txt', 'refresh');
    await page.getByRole('button', {name: 'Refresh files', exact: true}).click();
    await visible(ctx, button(page, 'new.txt'), 'new_file_observed');
  });
  await ctx.step('expected_result', async () => {
    for (const path of ['../', '.cma', 'cma/.cma']) {
      const denied = await raw(ctx, f, 'GET', '/tree', path, {allowed: [400, 403, 404, 422]});
      ctx.assert([400, 403, 404, 422].includes(denied.status), 'scope_boundary_refused');
    }
    const root = await raw(ctx, f, 'GET', '/tree', '', {am: other(f.lastWriteAm)});
    ctx.assert(!root.body.entries.some(entry => entry.name === '.cma'), 'private_runtime_tree_hidden');
  });
});

export const edit = define('edit', async ctx => {
  const f = await setup(ctx), {page} = f, path = f.root + '/edit.txt';
  await write(ctx, f, path, 'original');
  const editor = page.getByRole('textbox', {name: 'Contents of edit.txt', exact: true});
  await ctx.step('step_1', async () => {await open(ctx, f); await button(page, 'edit.txt').click();
    await editor.waitFor({state: 'visible'}); ctx.assert(await editor.inputValue() === 'original', 'original_bytes');});
  await ctx.step('step_2', async () => {await editor.fill('edited');
    await page.getByRole('button', {name: 'Save', exact: true}).click();
    await visible(ctx, page.getByText('File saved.', {exact: true}), 'save_acknowledged');});
  await ctx.step('step_3', async () => {
    const got = await raw(ctx, f, 'GET', '/content', path, {am: await observedPeer(ctx, f)});
    ctx.assert(Buffer.from(got.body.content_base64, 'base64').toString() === 'edited', 'durable_edit_bytes');
    await page.reload(); await open(ctx, f); await button(page, 'edit.txt').click();
    await editor.waitFor({state: 'visible'}); ctx.assert(await editor.inputValue() === 'edited', 'reopened_edit_bytes');
  });
  await ctx.step('step_4', async () => {
    const before = await read(ctx, f, path); await editor.fill('stale-draft');
    await write(ctx, f, path, 'newer-writer', {expected_sha256: before.body.sha256});
    const response = page.waitForResponse(r => r.request().method() === 'POST' && /\/files$/.test(new URL(r.url()).pathname));
    await page.getByRole('button', {name: 'Save', exact: true}).click();
    ctx.assert((await response).status() === 409 && await editor.inputValue() === 'stale-draft', 'stale_draft_retained');
  });
  await ctx.step('expected_result', async () => {
    const current = await read(ctx, f, path);
    ctx.assert(Buffer.from(current.body.content_base64, 'base64').toString() === 'newer-writer', 'newer_bytes_not_overwritten');
    await page.getByRole('button', {name: 'Reload file', exact: true}).click();
    await visible(ctx, page.getByText('Discard the unsaved changes to this file?', {exact: true}), 'discard_confirmation');
    await page.getByRole('button', {name: 'Cancel', exact: true}).click();
    ctx.assert(await editor.inputValue() === 'stale-draft', 'cancel_keeps_draft');
  });
});

export const remove = define('delete', async ctx => {
  const f = await setup(ctx), {page} = f;
  await write(ctx, f, f.root + '/delete.txt', 'delete'); await write(ctx, f, f.root + '/keep.txt', 'keep');
  await write(ctx, f, f.root + '/folder', '', {directory: true}); await write(ctx, f, f.root + '/folder/nested.txt', 'nested');
  await ctx.step('step_1', async () => {await open(ctx, f); await button(page, 'delete.txt').click();
    await visible(ctx, page.getByRole('textbox', {name: 'Contents of delete.txt', exact: true}), 'owned_file_selected');});
  await ctx.step('step_2', async () => {
    await page.getByRole('button', {name: 'Delete file', exact: true}).click();
    await visible(ctx, page.getByText('Delete /home/' + f.root + '/delete.txt? This cannot be undone.', {exact: true}), 'exact_delete_target');
    await page.getByRole('button', {name: 'Cancel', exact: true}).click();
    ctx.assert((await read(ctx, f, f.root + '/delete.txt')).status === 200, 'cancel_preserves_file');
    await page.getByRole('button', {name: 'Delete file', exact: true}).click();
    await page.getByRole('button', {name: 'Confirm', exact: true}).click();
    await button(page, 'delete.txt').waitFor({state: 'hidden'});
  });
  await ctx.step('step_3', async () => {
    await page.getByRole('button', {name: 'Refresh files', exact: true}).click();
    ctx.assert(await button(page, 'delete.txt').count() === 0, 'deleted_file_absent');
    await visible(ctx, button(page, 'keep.txt'), 'sibling_preserved');
  });
  await ctx.step('expected_result', async () => {
    await button(page, 'folder').click(); await page.getByRole('button', {name: 'Delete folder', exact: true}).click();
    await visible(ctx, page.getByText('Delete /home/' + f.root + '/folder and everything inside it? This cannot be undone.', {exact: true}), 'recursive_exact_confirmation');
    await page.getByRole('button', {name: 'Confirm', exact: true}).click();
    await button(page, 'folder').waitFor({state: 'hidden'});
    const tree = await raw(ctx, f, 'GET', '/tree', f.root, {am: await observedPeer(ctx, f)});
    ctx.assert(!tree.body.entries.some(row => ['folder', 'delete.txt'].includes(row.name))
      && tree.body.entries.some(row => row.name === 'keep.txt'), 'exact_deletion_scope');
    const denied = await raw(ctx, f, 'DELETE', '', '../escape', {allowed: [400, 403, 422]});
    ctx.assert(denied.status !== 200, 'delete_scope_escape_refused');
  });
});

export const download = define('download', async ctx => {
  const f = await setup(ctx, 'files-download'), {page} = f, text = 'harmless exact download';
  await write(ctx, f, f.root + '/known.txt', text);
  let bytes;
  await ctx.step('step_1', async () => {await open(ctx, f); await button(page, 'known.txt').click();
    const pending = page.waitForEvent('download'); await page.getByRole('button', {name: 'Download file', exact: true}).click();
    const value = await pending; bytes = await readFile(await value.path()); ctx.assert(value.suggestedFilename() === 'known.txt', 'download_filename');});
  await ctx.step('step_2', async () => {ctx.assert(hash(bytes) === hash(text), 'download_exact_bytes');});
  await ctx.step('step_3', async () => {
    const missing = await raw(ctx, f, 'GET', '/content', f.root + '/missing.txt', {allowed: [404]});
    ctx.assert(missing.status === 404, 'missing_file_refused');
  });
  await ctx.step('expected_result', async () => {
    const tooLarge = await raw(ctx, f, 'GET', '/content', f.large.path, {allowed: [400, 413, 422]});
    ctx.assert([400, 413, 422].includes(tooLarge.status), 'oversize_file_refused');
    const denied = await ctx.request({actor: 'outsider', am: other(ctx.am), agentId: f.agentId,
      method: 'GET', path: '/api/v1/agents/' + f.agentId + '/files/content?path=' + encodeURIComponent(f.root + '/known.txt'), allowed: [403, 404]});
    ctx.assert([403, 404].includes(denied.status), 'foreign_download_refused');
  });
});
