import {browser, navigateSettings, saveSettings, read, request, P, other, required, check,
  apiPath, equal, sameSettings} from './common.mjs';

export async function automaticPauseUI(ctx, state) {
  const session = await browser(ctx), saved = [];
  let attempts = 0, closeJob;
  const capture = async () => {
    const value = await ctx.observe('admin_auto_pause_browser', () => ctx.fixture('browser').evidence(session.page));
    required(value.authentication === 'browser_session', 'admin_auto_pause_cookie_session');
    check(Array.isArray(value.requests) && value.requests.length > 0
      && value.requests.every(r => r.request_id && ['a', 'b'].includes(r.serving_am)), 'admin_auto_pause_attribution');
    const writes = value.requests.filter(r => !['GET', 'HEAD', 'OPTIONS'].includes(r.method));
    check(writes.length <= attempts, 'admin_auto_pause_extra_write');
    for (const receipt of saved) check(writes.filter(r => r.request_id === receipt.request_id
      && r.method === 'PUT' && r.status === 200 && r.am === ctx.am && r.serving_am === ctx.am).length === 1,
    'admin_auto_pause_ui_ingress');
    return value;
  };
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    try {await capture();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_auto_pause_ui_finality');
  })();
  ctx.defer('admin_auto_pause_ui_close', close);
  return {close, async invalidBounds() {
    await navigateSettings(session); await capture();
    await session.page.getByLabel('Enable auto pause', {exact: true}).setChecked(false);
    const seconds = session.page.getByLabel('Pause after (seconds)', {exact: true});
    for (const invalid of [59, 86401]) {
      await seconds.fill(String(invalid));
      ctx.assert(!(await seconds.evaluate(node => node.checkValidity())), 'admin_auto_pause_ui_bounds');
      const before = await read(ctx, P + '/settings');
      const inactivity = {...before.settings.inactivity, auto_pause_enabled: false, pause_after_seconds: invalid};
      state.own({...before.settings, inactivity});
      const refused = await request(ctx, 'PATCH', P + '/settings', {expected_revision: before.revision, inactivity},
        {allowed: [422]});
      ctx.assert(refused.status === 422 && refused.body?.code === 'invalid_request', 'admin_auto_pause_invalid_refusal');
      ctx.assert(equal(before, await read(ctx, P + '/settings', other(ctx.am))), 'admin_auto_pause_invalid_effect');
    }
  }, async change(enabled, seconds, components = false) {
    await navigateSettings(session); await capture();
    const before = await read(ctx, P + '/settings');
    const desired = {...before.settings, inactivity: {...before.settings.inactivity,
      auto_pause_enabled: enabled, pause_after_seconds: seconds}};
    state.own(desired);
    await session.page.getByLabel('Enable auto pause', {exact: true}).setChecked(enabled);
    if (components) {
      await session.page.getByLabel('Pause after hours', {exact: true}).fill('0');
      await session.page.getByLabel('Pause after minutes', {exact: true}).fill('1');
      await session.page.getByLabel('Pause after seconds', {exact: true}).fill('0');
    } else await session.page.getByLabel('Pause after (seconds)', {exact: true}).fill(String(seconds));
    attempts++;
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === P + '/settings'),
      saveSettings(session),
    ]);
    const receipt = await ctx.observe('admin_auto_pause_ui_save', async () => ({status: response.status(),
      request_id: await response.headerValue('x-request-id'), body: await response.json()}));
    ctx.assert(receipt.status === 200 && receipt.request_id?.trim(), 'admin_auto_pause_save_response');
    ctx.assert(!saved.some(r => r.request_id === receipt.request_id), 'admin_auto_pause_duplicate_save');
    saved.push(receipt);
    ctx.assert(equal(response.request().postDataJSON(), {expected_revision: before.revision, settings: desired}),
      'admin_auto_pause_save_body');
    ctx.assert(Number.isSafeInteger(receipt.body.revision) && receipt.body.revision > before.revision
      && sameSettings(receipt.body.settings, desired), 'admin_auto_pause_saved_policy');
    ctx.assert(enabled ? typeof receipt.body.settings.inactivity.grace_started_at === 'string'
      && Number.isFinite(Date.parse(receipt.body.settings.inactivity.grace_started_at))
      : receipt.body.settings.inactivity.grace_started_at === null, 'admin_auto_pause_server_grace');
    await capture();
    for (const am of [ctx.am, other(ctx.am)]) {
      const peer = await read(ctx, P + '/settings', am);
      ctx.assert(peer.revision === receipt.body.revision && equal(peer.settings, receipt.body.settings),
        'admin_auto_pause_peer_policy');
    }
    return receipt.body;
  }};
}
