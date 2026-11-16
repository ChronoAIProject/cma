import {take, session, evidence, sendHeld, read, peerRead, settingsBody, equal,
  poll, sha, requireFixture} from './common.mjs';

const efforts = ['minimal', 'low', 'medium', 'high', 'x_high'];
const labels = ['Minimal', 'Low', 'Medium', 'High', 'Very high'];
export function chooseSettings(current, models) {
  requireFixture(Array.isArray(models) && models.some(m => m.available), 'configuration_available_model_required');
  const model = models.find(m => m.available && m.id !== current.model);
  requireFixture(model, 'configuration_alternate_available_model_required');
  requireFixture(efforts.includes(current.reasoning_effort), 'configuration_supported_effort_required');
  return {model: model.id, reasoning_effort: current.reasoning_effort === 'low' ? 'high' : 'low',
    permission_mode: current.permission_mode === 'guarded' ? 'autonomous' : 'guarded'};
}

async function expand(page) {
  const button = page.getByRole('button', {name: /^Agent settings:/});
  if (await button.getAttribute('aria-expanded') !== 'true') await button.click();
  const region = page.getByRole('region', {name: 'Agent settings', exact: true});
  await region.waitFor({state: 'visible'}); return region;
}
async function permission(page, value) {
  await page.getByRole('button', {name: 'Permission mode', exact: true}).click();
  await page.getByRole('menuitemradio', {name: value === 'guarded' ? 'Guarded' : 'Autonomous', exact: true}).click();
}
export const settings = {
  id: 'C275:cma-user.agent.session-settings', scope: 'agent',
  steps: ['step_1', 'step_2', 'step_3', 'step_4', 'expected_result'], fixtures: ['user-fixtures', 'browser'],
  async run(ctx) {
    let resource, ui, before, models, desired;
    await ctx.step('step_1', async () => {
      resource = await take(ctx, 'agent-session-settings'); ui = await session(ctx, resource);
      before = await read(ctx, resource, '/settings');
      ctx.assert((await read(ctx, resource)).status === 'ready' && before.pending === false, 'settings_initial_idle_required');
      await expand(ui.page);
      ctx.assert(await ui.page.getByRole('button', {name: 'Permission mode', exact: true}).isEnabled(),
        'settings_idle_controls_disabled');
    });
    await ctx.step('step_2', async () => {
      models = (await read(ctx, resource, '/models')).models;
      desired = chooseSettings(before, models);
      await permission(ui.page, desired.permission_mode);
      await ui.page.getByRole('button', {name: /^Model and reasoning effort:/}).click();
      const card = ui.page.getByRole('dialog', {name: 'Model and reasoning effort', exact: true});
      const modelButton = card.getByRole('button', {name: 'Model', exact: true});
      await modelButton.waitFor({state: 'visible'}); await modelButton.click();
      const menu = ui.page.getByRole('menu', {name: 'Model', exact: true});
      const expected = [...models.map(m => m.id), ...(!models.some(m => m.id === before.model) ? [before.model] : [])];
      ctx.assert(equal((await menu.getByRole('menuitemradio').evaluateAll(nodes => nodes.map(n => n.dataset.id))).sort(), expected.sort()),
        'settings_ui_catalog_does_not_match_api');
      for (const model of models) {
        const option = menu.locator('[role="menuitemradio"][data-id=' + JSON.stringify(model.id) + ']');
        const disabled = await option.getAttribute('aria-disabled') === 'true' || await option.isDisabled();
        ctx.assert(disabled === !model.available, 'settings_unavailable_model_selectable');
      }
      await menu.locator('[role="menuitemradio"][data-id=' + JSON.stringify(desired.model) + ']').click();
      const slider = card.getByRole('slider', {name: 'Reasoning effort', exact: true});
      ctx.assert(await slider.getAttribute('aria-valuemin') === '0' && await slider.getAttribute('aria-valuemax') === '4',
        'settings_effort_choices_mismatch');
      await slider.press('Home');
      for (let i = 0; i < efforts.indexOf(desired.reasoning_effort); i++) await slider.press('ArrowRight');
      ctx.assert(await slider.getAttribute('aria-valuetext') === labels[efforts.indexOf(desired.reasoning_effort)],
        'settings_effort_selection_mismatch');
      await ui.page.keyboard.press('Escape');
      ctx.assert(!equal(desired, settingsBody(before)), 'settings_noop_not_coverage');
    });
    await ctx.step('step_3', async () => {
      const response = ui.page.waitForResponse(r => r.request().method() === 'PATCH'
        && new URL(r.url()).pathname.endsWith('/settings'));
      await ui.page.getByRole('button', {name: 'Apply settings', exact: true}).click();
      const actual = await response;
      ctx.assert(actual.status() === 200 && equal(actual.request().postDataJSON(), desired), 'settings_ui_update_not_acknowledged');
      const value = await actual.json();
      ctx.assert(equal(settingsBody(value), desired), 'settings_acknowledged_value_mismatch');
      if (value.pending) {
        await ui.page.getByText('Waiting for the runtime to confirm these settings.', {exact: true}).waitFor({state: 'visible'});
        ctx.assert(await ui.page.getByText('Waiting for the runtime to confirm these settings.', {exact: true}).isVisible(),
          'settings_pending_status_missing');
      }
    });
    await ctx.step('step_4', async () => {
      const confirmed = await poll(() => read(ctx, resource, '/settings'), s => s.pending === false
        && equal(settingsBody(s), desired), 'settings_runtime_confirmation_timeout');
      ctx.assert(equal(settingsBody(confirmed), desired), 'settings_not_effective_before_turn');
      ctx.assert(equal(settingsBody(await peerRead(ctx, resource, '/settings')), desired), 'settings_peer_not_persistent');
      await ui.page.reload(); await expand(ui.page);
      ctx.assert((await ui.page.getByRole('button', {name: /^Agent settings:/}).getAttribute('aria-label'))
        .includes(labels[efforts.indexOf(desired.reasoning_effort)]), 'settings_reload_summary_missing');
    });
    await ctx.step('expected_result', async () => {
      requireFixture(typeof resource.controls.requestMetadata === 'function', 'configuration_provider_probe_not_installed');
      const held = await sendHeld(ctx, resource, ui, 'settings-effect');
      const metadata = await ctx.observe('configuration_next_turn_settings_metadata', () =>
        resource.controls.requestMetadata({caseId: held.caseId}));
      ctx.assert(metadata.length === 1 && metadata[0].case_id_sha256 === sha(held.caseId)
        && metadata[0].model === desired.model && metadata[0].reasoning_effort === desired.reasoning_effort,
      'settings_not_applied_to_actual_model_request');
      await expand(ui.page);
      await permission(ui.page, before.permission_mode);
      const apply = ui.page.getByRole('button', {name: 'Apply settings', exact: true});
      ctx.assert(await apply.isDisabled(), 'settings_busy_apply_enabled');
      ctx.assert(await ui.page.getByText('Settings can be changed after the current turn finishes.', {exact: true}).isVisible(),
        'settings_busy_reason_missing');
      ctx.assert(equal(settingsBody(await read(ctx, resource, '/settings')), desired), 'settings_busy_draft_mutated_runtime');
      await held.finish();
      ctx.assert(equal(settingsBody(await peerRead(ctx, resource, '/settings')), desired), 'settings_changed_after_held_turn');
      await evidence(ctx, ui);
    });
  },
};
