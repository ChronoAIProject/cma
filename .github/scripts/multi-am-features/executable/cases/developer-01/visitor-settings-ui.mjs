import {browser, evidence, navigateSettings, saveSettings, required, sameSettings, read, other, apiPath, P} from './common.mjs';

// The browser owns one settings intent. Close/drain before restoration so an
// in-flight UI write cannot silently overwrite the original policy afterwards.
export async function visitorSettingsUI(ctx, state) {
  const session = await browser(ctx);
  let savedId, closeJob;
  async function observe() {
    const receipt = await evidence(ctx, session);
    required(receipt.authentication === 'browser_session', 'admin_visitor_cookie_session_required');
    const writes = receipt.requests.filter(r => !['GET', 'HEAD', 'OPTIONS'].includes(r.method));
    ctx.assert(writes.length <= 1, 'admin_visitor_browser_extra_write');
    if (savedId) ctx.assert(writes.length === 1 && writes[0].method === 'PUT' && writes[0].status === 200
      && writes[0].request_id === savedId && writes[0].am === writes[0].serving_am,
    'admin_visitor_browser_save_receipt');
    return writes[0];
  }
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    // Always retain drained evidence even when browser shutdown failed.
    try {await observe();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_visitor_browser_cleanup_failed');
  })();
  ctx.defer('admin_visitor_browser_finality', close);
  return {close, async change() {
    await navigateSettings(session);
    await observe();
    const desired = structuredClone(state.original.settings);
    desired.limits.visitor_max_sandboxes = 1;
    desired.limits.visitor_max_launches_per_day = 2;
    required(!sameSettings(desired, state.original.settings), 'admin_visitor_changed_policy_fixture_required');
    state.own(desired);
    await session.page.getByLabel('Visitor Sandboxes', {exact: true}).fill('1');
    await session.page.getByLabel('Visitor launches per day', {exact: true}).fill('2');
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === P + '/settings'),
      saveSettings(session),
    ]);
    savedId = await response.headerValue('x-request-id');
    ctx.assert(response.status() === 200 && savedId?.trim(), 'admin_visitor_browser_save_response');
    const write = await observe();
    const changed = await read(ctx, P + '/settings', other(write.serving_am));
    ctx.assert(changed.revision > state.original.revision && sameSettings(changed.settings, desired),
      'admin_visitor_policy_not_changed');
    return changed;
  }};
}
