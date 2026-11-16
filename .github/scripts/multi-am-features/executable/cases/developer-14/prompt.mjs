import {take, session, evidence, openPreferences, savePreferences, sendHeld, read, request,
  peerRead, unique, poll, sha, requireFixture, conflictPreferences} from './common.mjs';

export async function promptEffect(ctx, resource, turn, expected) {
  requireFixture(typeof resource.controls.requestMetadata === 'function', 'configuration_provider_probe_not_installed');
  const rows = await ctx.observe('configuration_next_turn_prompt_metadata', () =>
    resource.controls.requestMetadata({caseId: turn.caseId}));
  ctx.assert(Array.isArray(rows) && rows.length === 1 && rows[0].case_id_sha256 === sha(turn.caseId),
    'configuration_provider_request_identity');
  const marker = expected.match(/PROMPT-[a-f0-9-]{36}/)?.[0];
  ctx.assert(!!marker && rows[0].instruction_marker_sha256?.includes(sha(marker)),
    'configuration_prompt_not_in_actual_next_turn');
}

export const prompt = {
  id: 'C275:cma-user.agent.system-prompt', scope: 'agent',
  steps: ['step_1', 'step_2', 'step_3', 'expected_result'], fixtures: ['user-fixtures', 'browser'],
  async run(ctx) {
    let resource, ui, before, desired, editor, update;
    await ctx.step('step_1', async () => {
      resource = await take(ctx, 'agent-system-prompt');
      requireFixture(typeof resource.controls.requestMetadata === 'function', 'configuration_provider_probe_not_installed');
      ui = await session(ctx, resource); before = await read(ctx, resource, '/preferences');
      ctx.assert((await read(ctx, resource)).status === 'ready' && before.prompt_supported && !before.prompt_pending,
        'prompt_initial_state_not_idle_supported');
      editor = await openPreferences(ui.page, 'prompt');
      ctx.assert(await editor.field.isEnabled(), 'prompt_idle_editor_disabled');
    });
    await ctx.step('step_2', async () => {
      await editor.field.fill('é'.repeat(16385));
      ctx.assert(await editor.save.isDisabled(), 'prompt_byte_limit_not_enforced');
      ctx.assert(await editor.dialog.getByText('Keep the system prompt within 32 KiB.', {exact: true}).isVisible(),
        'prompt_byte_limit_not_explained');
      desired = 'For subsequent turns keep this exact instruction marker: ' + unique('PROMPT');
      await editor.field.fill(desired); update = await savePreferences(ui.page, editor);
      ctx.assert(update.request.expected_revision === before.revision && update.request.system_prompt === desired,
        'prompt_ui_request_mismatch');
      ctx.assert(update.body.system_prompt === desired && update.body.revision > before.revision,
        'prompt_acknowledgement_mismatch');
    });
    await ctx.step('step_3', async () => {
      if (update.body.prompt_pending) {
        await editor.dialog.getByText('Saved. The Agent is applying the updated system prompt.', {exact: true})
          .waitFor({state: 'visible'});
        ctx.assert(await editor.save.isDisabled(), 'prompt_pending_allows_another_save');
      }
      const seen = await poll(() => read(ctx, resource, '/preferences'), p => p.system_prompt === desired
        && p.prompt_pending === false, 'prompt_runtime_confirmation_timeout');
      ctx.assert(seen.revision === update.body.revision, 'prompt_confirmed_revision_changed');
      if (await editor.dialog.isVisible()) await editor.dialog.getByRole('button', {name: 'Cancel', exact: true}).click();
      const turn = await sendHeld(ctx, resource, ui, 'prompt-effect');
      await promptEffect(ctx, resource, turn, desired); await turn.finish();
      ctx.assert((await peerRead(ctx, resource, '/preferences')).system_prompt === desired, 'prompt_peer_value_missing');
    });
    await ctx.step('expected_result', async () => {
      // A real held provider stream makes the Agent busy. No fabricated AM or
      // browser responses are used to manufacture the refusal.
      const held = await sendHeld(ctx, resource, ui, 'prompt-busy');
      await ui.page.getByRole('button', {name: 'More Agent actions', exact: true}).click();
      const item = ui.page.locator('[role="menuitem"][data-id="update-prompt"]');
      ctx.assert(await item.getAttribute('aria-disabled') === 'true' || await item.isDisabled(),
        'prompt_busy_menu_not_disabled');
      const snapshot = await read(ctx, resource, '/preferences');
      const refused = await request(ctx, resource, 'PATCH', '/preferences',
        {expected_revision: snapshot.revision, system_prompt: unique('Busy refusal')}, [409], unique('busy-prompt'));
      ctx.assert(refused.body.code === 'agent_not_ready', 'prompt_busy_refusal_not_typed');
      ctx.assert((await read(ctx, resource, '/preferences')).system_prompt === desired, 'prompt_busy_refusal_mutated');
      await ui.page.keyboard.press('Escape'); await held.finish();
      const stale = await openPreferences(ui.page, 'prompt');
      const revision = (await read(ctx, resource, '/preferences')).revision;
      const winner = unique('Concurrent title');
      await stale.field.fill(unique('Stale prompt'));
      const {conflict, winner: changed} = await conflictPreferences(ui.page, stale, body =>
        request(ctx, resource, 'PATCH', '/preferences',
          {expected_revision: body.expected_revision, title: winner}, [200], unique('prompt-revision')));
      ctx.assert(changed.body.revision > revision, 'prompt_concurrent_revision_not_advanced');
      ctx.assert(conflict.request.expected_revision === revision, 'prompt_stale_revision_not_exercised');
      await stale.dialog.getByRole('button', {name: 'Reload preferences', exact: true}).waitFor({state: 'visible'});
      ctx.assert(await stale.save.isDisabled(), 'prompt_conflict_did_not_require_reload');
      await stale.dialog.getByRole('button', {name: 'Reload preferences', exact: true}).click();
      await ui.page.waitForFunction(value => document.querySelector('textarea[aria-label="Agent system prompt"]')?.value === value,
        desired, {polling: 2000});
      ctx.assert((await read(ctx, resource, '/preferences')).system_prompt === desired, 'prompt_stale_revision_mutated');
      await stale.dialog.getByRole('button', {name: 'Cancel', exact: true}).click();
      await evidence(ctx, ui);
      await ui.close();
      await resource.cleanup();
      // This is a separately provisioned real runtime with a declared immutable
      // image. Missing capability variant remains BLOCKED for the whole row.
      const unsupported = await take(ctx, 'agent-system-prompt-unsupported');
      const alternate = await session(ctx, unsupported), unavailable = await openPreferences(alternate.page, 'prompt');
      ctx.assert((await read(ctx, unsupported, '/preferences')).prompt_supported === false,
        'prompt_unsupported_runtime_not_exercised');
      ctx.assert(await unavailable.field.isDisabled() && await unavailable.save.isDisabled(),
        'prompt_unsupported_write_enabled');
      ctx.assert(await unavailable.dialog.getByText('This runtime does not support system prompt updates yet.', {exact: true}).isVisible(),
        'prompt_unsupported_reason_missing');
      await evidence(ctx, alternate, 'configuration_unsupported_browser_evidence');
    });
  },
};
