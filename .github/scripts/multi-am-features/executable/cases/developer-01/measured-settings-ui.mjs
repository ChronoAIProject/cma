import {browser, navigateSettings, saveSettings, read, P, apiPath, required, check, equal, other} from './common.mjs';

export async function measuredSettingsUI(ctx, state) {
  const session = await browser(ctx), saves = [];
  let attempts = 0, closeJob;
  const capture = async () => {
    const value = await ctx.observe('admin_measured_settings_browser', () => ctx.fixture('browser').evidence(session.page));
    required(value.authentication === 'browser_session', 'admin_measured_cookie_session');
    check(Array.isArray(value.requests), 'admin_measured_settings_receipts');
    const writes = value.requests.filter(r => !['GET', 'HEAD', 'OPTIONS'].includes(r.method));
    check(writes.length <= attempts, 'admin_measured_extra_settings_write');
    for (const saved of saves) check(writes.filter(r => r.request_id === saved.request_id && r.status === 200
      && r.method === 'PUT' && r.am === ctx.am && r.serving_am === ctx.am).length === 1,
    'admin_measured_settings_ingress');
  };
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    try {await capture();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_measured_settings_finality');
  })();
  ctx.defer('admin_measured_settings_close', close);
  return {close, async change(enabled) {
    await navigateSettings(session); await capture();
    const before = await read(ctx, P + '/settings'), desired = structuredClone(before.settings);
    desired.usage_reporting.measured_visible = enabled; state.own(desired);
    await session.page.getByLabel('Show measured usage', {exact: true}).setChecked(enabled);
    attempts++;
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === P + '/settings'),
      saveSettings(session),
    ]);
    const saved = await ctx.observe('admin_measured_settings_save', async () => ({status: response.status(),
      request_id: await response.headerValue('x-request-id'), body: await response.json()}));
    ctx.assert(saved.status === 200 && saved.request_id?.trim() && !saves.some(r => r.request_id === saved.request_id),
      'admin_measured_settings_response'); saves.push(saved);
    ctx.assert(equal(response.request().postDataJSON(), {expected_revision: before.revision, settings: desired}),
      'admin_measured_settings_body');
    ctx.assert(Number.isSafeInteger(saved.body.revision) && saved.body.revision > before.revision
      && equal(saved.body.settings, desired), 'admin_measured_saved_policy');
    await capture();
    for (const am of [ctx.am, other(ctx.am)]) {
      const actual = await read(ctx, P + '/settings', am);
      ctx.assert(actual.revision === saved.body.revision && equal(actual.settings, desired), 'admin_measured_peer_policy');
    }
    return saved.body;
  }};
}
