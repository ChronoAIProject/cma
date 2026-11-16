import {take, session, evidence, openPreferences, savePreferences, sendHeld,
  read, request, peerRead, unique, equal, id, other, conflictPreferences} from './common.mjs';

export const rename = {
  id: 'C275:cma-user.agent.rename', scope: 'agent',
  steps: ['step_1', 'step_2', 'step_3', 'expected_result'], fixtures: ['user-fixtures', 'browser'],
  async run(ctx) {
    let resource, ui, editor, before, baseline, turn, composer, desired, draft, updated;
    await ctx.step('step_1', async () => {
      resource = await take(ctx, 'agent-rename'); ui = await session(ctx, resource);
      turn = await sendHeld(ctx, resource, ui, 'rename-history'); await turn.finish();
      baseline = await read(ctx, resource, '/transcript');
      ctx.assert(Array.isArray(baseline.turns) && baseline.turns.length > 0, 'rename_real_conversation_required');
      draft = unique('Retain draft');
      const field = ui.page.getByRole('textbox', {name: 'Message', exact: true});
      await field.fill(draft); composer = await field.elementHandle();
      before = await read(ctx, resource, '/preferences');
      editor = await openPreferences(ui.page, 'rename');
      ctx.assert(await editor.field.inputValue() === before.title, 'rename_initial_title_mismatch');
    });
    await ctx.step('step_2', async () => {
      await editor.field.fill(''); ctx.assert(await editor.save.isDisabled(), 'rename_empty_not_rejected');
      await editor.field.fill('x'.repeat(65)); ctx.assert(await editor.save.isDisabled(), 'rename_overlength_not_rejected');
      desired = unique('C275 rename');
      ctx.assert(desired.length <= 64 && desired !== before.title, 'rename_unique_valid_value_required');
      await editor.field.fill(desired);
      updated = await savePreferences(ui.page, editor);
      ctx.assert(updated.request.expected_revision === before.revision && updated.request.title === desired,
        'rename_ui_revision_request_mismatch');
      ctx.assert(updated.body.title === desired && updated.body.revision > before.revision, 'rename_acknowledgement_mismatch');
      await editor.dialog.waitFor({state: 'hidden'});
      ctx.assert(await composer.evaluate(node => node.isConnected && node === document.querySelector('#agent-message')),
        'rename_remounted_conversation');
      ctx.assert(await ui.page.getByRole('textbox', {name: 'Message', exact: true}).inputValue() === draft,
        'rename_lost_draft');
      ctx.assert(equal((await read(ctx, resource, '/transcript')).turns, baseline.turns), 'rename_changed_conversation');
    });
    await ctx.step('step_3', async () => {
      const peer = await peerRead(ctx, resource, '/preferences');
      ctx.assert(peer.title === desired && peer.revision === updated.body.revision, 'rename_peer_persistence_failed');
      await ui.switchAM(other(ctx.am)); await ui.page.reload();
      await ui.page.getByRole('heading', {name: desired, exact: true}).waitFor({state: 'visible'});
      const show = ui.page.getByRole('button', {name: 'Show sidebar', exact: true});
      if (await show.isVisible()) await show.click();
      const nav = ui.page.locator(`a[href="/agents/${id(resource.agentId)}"]`).first();
      await nav.waitFor({state: 'visible'});
      ctx.assert((await nav.innerText()).includes(desired), 'rename_navigation_title_not_updated');
      ctx.assert(equal((await read(ctx, resource, '/transcript')).turns, baseline.turns), 'rename_reload_lost_history');
    });
    await ctx.step('expected_result', async () => {
      const stale = await openPreferences(ui.page, 'rename');
      const staleRevision = (await read(ctx, resource, '/preferences')).revision;
      const competing = unique('Concurrent rename');
      await stale.field.fill(unique('Stale rename'));
      const {conflict: refused, winner: win} = await conflictPreferences(ui.page, stale, body =>
        request(ctx, resource, 'PATCH', '/preferences',
          {expected_revision: body.expected_revision, title: competing}, [200], unique('rename-concurrent')));
      ctx.assert(win.body.title === competing && win.body.revision > staleRevision, 'rename_concurrent_write_not_committed');
      ctx.assert(refused.request.expected_revision === staleRevision, 'rename_stale_revision_not_exercised');
      await stale.dialog.getByRole('button', {name: 'Reload preferences', exact: true}).waitFor({state: 'visible'});
      ctx.assert(await stale.save.isDisabled(), 'rename_conflict_did_not_require_reload');
      ctx.assert((await read(ctx, resource, '/preferences')).title === competing, 'rename_stale_write_changed_title');
      await stale.dialog.getByRole('button', {name: 'Reload preferences', exact: true}).click();
      await ui.page.waitForFunction(value => document.querySelector('input[aria-label="Agent name"]')?.value === value,
        competing, {polling: 2000});
      await stale.field.fill(desired);
      const restored = await savePreferences(ui.page, stale);
      ctx.assert(restored.request.expected_revision === win.body.revision && restored.body.title === desired,
        'rename_reloaded_revision_not_applied');
      ctx.assert(equal((await peerRead(ctx, resource, '/transcript')).turns, baseline.turns), 'rename_conflict_changed_history');
      await evidence(ctx, ui);
    });
  },
};
