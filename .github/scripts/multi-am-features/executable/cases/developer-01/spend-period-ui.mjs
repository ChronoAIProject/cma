import {browser, navigateSettings, saveSettings, read, P, apiPath, other, equal, sameSettings, required, check} from './common.mjs';
import {operationsPrefetch} from './operations-prefetch.mjs';
import {periodFields} from './spend-period-data.mjs';

export async function spendPeriodUI(ctx, state) {
  const session = await browser(ctx), reports = operationsPrefetch(ctx, session);
  let attempted = false, saved, closeJob;
  const capture = async (final = false) => {
    const proof = await reports.capture('admin_period_browser', final);
    required(proof.authentication === 'browser_session', 'admin_period_cookie_session');
    const writes = proof.requests.filter(r => !['GET', 'HEAD', 'OPTIONS'].includes(r.method));
    check(writes.length <= (attempted ? 1 : 0), 'admin_period_extra_write');
    if (saved) check(writes.length === 1 && writes[0].request_id === saved.request_id
      && writes[0].status === 200 && writes[0].method === 'PUT' && writes[0].am === ctx.am && writes[0].serving_am === ctx.am,
    'admin_period_save_ingress');
  };
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    try {await capture(true);} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_period_browser_finality');
  })();
  ctx.defer('admin_period_close', close);
  const field = key => session.page.getByLabel(periodFields[key], {exact: true});
  const fill = async value => {
    for (const key of Object.keys(periodFields)) {
      await field(key).fill(String(value[key]));
      ctx.assert(await field(key).inputValue() === String(value[key]), 'admin_period_exact_draft');
    }
  };
  const desired = (before, period) => ({...before.settings, model_budget: {...before.settings.model_budget,
    platform: {...before.settings.model_budget.platform, ...period}}});
  return {close, async open() {
    await navigateSettings(session); await capture();
    const configure = session.page.getByRole('button', {name: 'Configure platform period', exact: true});
    if (await configure.isVisible()) await configure.click();
  }, async invalid({field: key, error, value}) {
    const before = await read(ctx, P + '/settings');
    await fill(value); state.own(desired(before, value));
    await session.page.getByRole('button', {name: 'Save settings', exact: true}).click();
    await capture();
    ctx.assert(await field(key).inputValue() === String(value[key]), 'admin_period_invalid_text_retained');
    const problem = await ctx.observe('admin_period_field_error', () => field(key).evaluate(node => {
      const describedBy = node.getAttribute('aria-describedby');
      return {invalid: node.getAttribute('aria-invalid'), describedBy,
        text: describedBy ? node.ownerDocument.getElementById(describedBy)?.textContent?.trim() : null};
    }));
    ctx.assert(problem.invalid === 'true' && typeof problem.describedBy === 'string' && problem.describedBy.trim()
      && problem.text === error, 'admin_period_field_error');
    for (const am of [ctx.am, other(ctx.am)]) ctx.assert(equal(before, await read(ctx, P + '/settings', am)),
      'admin_period_invalid_effect');
  }, async save(period) {
    const before = await read(ctx, P + '/settings'), target = desired(before, period); state.own(target);
    ctx.assert(!sameSettings(target, before.settings), 'admin_period_save_changes_value');
    await fill(period); attempted = true;
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === P + '/settings'), saveSettings(session),
    ]);
    saved = await ctx.observe('admin_period_save', async () => ({status: response.status(),
      request_id: await response.headerValue('x-request-id'), body: await response.json()}));
    ctx.assert(saved.status === 200 && saved.request_id?.trim(), 'admin_period_save_response');
    ctx.assert(equal(response.request().postDataJSON(), {expected_revision: before.revision, settings: target}), 'admin_period_save_body');
    ctx.assert(Number.isSafeInteger(saved.body.revision) && saved.body.revision > before.revision
      && sameSettings(saved.body.settings, target), 'admin_period_saved_policy');
    await capture();
    for (const am of [ctx.am, other(ctx.am)]) ctx.assert(equal(saved.body, await read(ctx, P + '/settings', am)),
      'admin_period_peer_policy');
    return saved.body;
  }};
}
