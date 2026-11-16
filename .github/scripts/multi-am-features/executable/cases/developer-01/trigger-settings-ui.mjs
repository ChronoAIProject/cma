import {browser, check, navigateSettings, saveSettings, read, other, required, apiPath, P, sameSettings} from './common.mjs';
import {operationsPrefetch} from './operations-prefetch.mjs';

export async function triggerSettingsUI(ctx, state) {
  const session = await browser(ctx), saved = [];
  const reports = operationsPrefetch(ctx, session);
  let attempts = 0, closeJob;
  const capture = async (final = false) => {
    const value = await reports.capture('admin_browser_evidence', final);
    required(value.authentication === 'browser_session', 'admin_trigger_cookie_session_required');
    const writes = value.requests.filter(r => !['GET', 'HEAD', 'OPTIONS'].includes(r.method));
    check(writes.length <= attempts, 'admin_trigger_browser_extra_write');
    for (const receipt of saved) check(writes.filter(r => r.request_id === receipt.request_id
      && r.method === 'PUT' && r.status === 200 && r.am === ctx.am && r.serving_am === ctx.am).length === 1,
    'admin_trigger_browser_ingress');
    return value;
  };
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    try {await capture(true);} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_trigger_browser_finality_failed');
  })();
  ctx.defer('admin_trigger_browser_finality', close);
  return {close, async change(enabled, paused) {
    await navigateSettings(session);
    await capture();
    const before = await read(ctx, P + '/settings');
    const desired = {...before.settings, triggers: {...before.settings.triggers,
      enabled, orchestration_paused: paused}};
    state.own(desired);
    await session.page.getByLabel('Triggers enabled', {exact: true}).setChecked(enabled);
    await session.page.getByLabel('Orchestration paused', {exact: true}).setChecked(paused);
    attempts++;
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === P + '/settings'),
      saveSettings(session),
    ]);
    const receipt = await ctx.observe('admin_trigger_ui_save', async () => ({status: response.status(),
      request_id: await response.headerValue('x-request-id')}));
    ctx.assert(receipt.status === 200 && receipt.request_id?.trim(), 'admin_trigger_save_response');
    ctx.assert(!saved.some(r => r.request_id === receipt.request_id), 'admin_trigger_save_duplicate');
    saved.push(receipt);
    await capture();
    const peer = await read(ctx, P + '/settings', other(ctx.am));
    ctx.assert(peer.revision > before.revision && sameSettings(peer.settings, desired), 'admin_trigger_settings_peer');
    return peer;
  }};
}
