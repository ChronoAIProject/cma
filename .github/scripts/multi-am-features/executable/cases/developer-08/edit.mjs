import {define, take, open, docPage, edit, save, source, read, other, path, visible, downloadText, hash} from './helpers.mjs';
import {randomUUID} from 'node:crypto';

export const editCase = define('edit', async ctx => {
  let fixture, session, before, saved;
  const title = `Edited ${randomUUID()}`, text = `Changed content ${randomUUID()}`;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx); session = await open(ctx, fixture.actors.user.actor);
    await docPage(ctx, session, fixture.document); before = await read(ctx, fixture.document, fixture.actors.user.actor);
    ctx.assert(before.access === 'owner', 'document_edit_requires_owner');
  });
  await ctx.step('step_2', async () => {
    await edit(session, title, text);
    ctx.assert(await session.page.getByRole('textbox', {name: 'Document title', exact: true}).inputValue() === title,
      'document_title_edited');
    ctx.assert((await session.page.getByRole('textbox', {name: 'Document content', exact: true}).innerText()).includes(text),
      'document_content_edited');
  });
  await ctx.step('step_3', async () => {
    saved = await save(ctx, session, fixture.document);
    ctx.assert(saved.revision > before.revision && saved.source_revision > before.source_revision, 'document_revision_advanced');
  });
  await ctx.step('step_4', async () => {
    await session.switchAM(other(ctx.am)); await session.page.reload();
    await visible(ctx, session.page.getByRole('textbox', {name: 'Document title', exact: true}), 'document_reloaded');
    ctx.assert(await session.page.getByRole('textbox', {name: 'Document title', exact: true}).inputValue() === title,
      'document_saved_title_retained');
    ctx.assert((await session.page.getByRole('textbox', {name: 'Document content', exact: true}).innerText()).includes(text),
      'document_saved_content_retained');
  });
  await ctx.step('expected_result', async () => {
    const current = await read(ctx, fixture.document, fixture.actors.user.actor);
    const content = await source(ctx, fixture.document, fixture.actors.user.actor);
    ctx.assert(current.revision === saved.revision && current.sha256 === hash(content.markdown)
      && content.revision === saved.source_revision && content.markdown.includes(text), 'document_saved_source_matches');
    const denied = await ctx.request({actor: fixture.actors.user.actor, am: ctx.am, method: 'PATCH',
      path: path(fixture.document.id), key: randomUUID(), allowed: [409],
      body: {expected_revision: before.revision, markdown: 'Stale writer must not win'}});
    ctx.assert(denied.status === 409 && (await source(ctx, fixture.document, fixture.actors.user.actor)).markdown === content.markdown,
      'document_stale_revision_refused');
  });
});

export const conflictCase = define('conflict-draft', async ctx => {
  let fixture, first, second, latest, draft;
  const staleText = `Retained draft ${randomUUID()}`, freshText = `Winning revision ${randomUUID()}`;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx); first = await open(ctx, fixture.actors.user.actor);
    second = await open(ctx, fixture.actors.user.actor, other(ctx.am));
    await docPage(ctx, first, fixture.document); await docPage(ctx, second, fixture.document);
    await edit(first, 'Local retained draft', staleText);
    await edit(second, 'Peer saved revision', freshText);
    latest = await save(ctx, second, fixture.document);
    ctx.assert(latest.source_revision > fixture.document.source_revision, 'document_peer_write_committed');
  });
  await ctx.step('step_2', async () => {
    await save(ctx, first, fixture.document, 409);
    await visible(ctx, first.page.getByText('Version changed', {exact: true}), 'document_conflict_visible');
    ctx.assert((await first.page.getByRole('textbox', {name: 'Document content', exact: true}).innerText()).includes(staleText),
      'document_conflict_preserves_local_work');
  });
  await ctx.step('step_3', async () => {
    const [download] = await Promise.all([first.page.waitForEvent('download'),
      first.page.getByRole('button', {name: 'Download draft', exact: true}).click()]);
    ctx.defer('delete_document_draft_download', () => download.delete());
    draft = await downloadText(download);
    ctx.assert(draft.includes(staleText) && !draft.includes(freshText), 'document_download_is_retained_draft');
    await ctx.observe('document_draft_download', async () => ({sha256: hash(draft), bytes: Buffer.byteLength(draft)}));
  });
  await ctx.step('step_4', async () => {
    await first.page.getByRole('button', {name: 'Reload latest', exact: true}).click();
    const dialog = first.page.getByRole('dialog', {name: 'Load the latest document?'});
    await visible(ctx, dialog, 'document_discard_requires_confirmation');
    await first.page.keyboard.press('Escape'); await dialog.waitFor({state: 'hidden'});
    ctx.assert((await first.page.getByRole('textbox', {name: 'Document content', exact: true}).innerText()).includes(staleText),
      'document_cancel_discard_keeps_draft');
    await first.page.getByRole('button', {name: 'Reload latest', exact: true}).click();
    await dialog.getByRole('button', {name: 'Discard draft and reload', exact: true}).click();
    await visible(ctx, first.page.locator('.document-workspace__save-status').filter({hasText: /^Saved$/}), 'document_latest_loaded');
    ctx.assert((await first.page.getByRole('textbox', {name: 'Document content', exact: true}).innerText()).includes(freshText),
      'document_explicit_discard_loads_latest');
  });
  await ctx.step('expected_result', async () => {
    for (const am of [ctx.am, other(ctx.am)]) {
      const current = await read(ctx, fixture.document, fixture.actors.user.actor, am);
      const content = await source(ctx, fixture.document, fixture.actors.user.actor, am);
      ctx.assert(current.source_revision === latest.source_revision && content.markdown.includes(freshText)
        && !content.markdown.includes(staleText), 'document_newer_revision_intact');
    }
  });
});
