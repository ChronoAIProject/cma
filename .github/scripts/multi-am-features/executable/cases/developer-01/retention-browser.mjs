import {browser, navigateSettings, saveSettings, read, P, apiPath, required, check, equal, sameSettings, other} from './common.mjs';

const SETTINGS = P + '/settings', REPORTS = P + '/usage/metrics/reports';
export async function retentionBrowser(ctx, state) {
  const session = await browser(ctx), writes = [], pending = new Set(), failures = [], byRequest = new Map();
  let attempted = false, saved, closeJob;
  const onRequest = req => {
    try {
      if (['GET', 'HEAD', 'OPTIONS'].includes(req.method())) return;
      check(writes.length < 256, 'admin_retention_request_bound');
      const row = {method: req.method(), path: apiPath(req.url())};
      if (row.path === SETTINGS || row.path === REPORTS) row.body = req.postDataJSON();
      writes.push(row); byRequest.set(req, row);
    } catch (error) {failures.push(error);}
  };
  const onResponse = response => {
    const row = byRequest.get(response.request()); if (!row) return;
    const task = (async () => {
      const body = await response.json();
      row.response = {status: response.status(), request_id: await response.headerValue('x-request-id'),
        body: row.path === REPORTS ? {report_id: body.report_id, status: body.status} : body};
    })();
    pending.add(task);
    void task.then(() => pending.delete(task), error => {failures.push(error); pending.delete(task);});
  };
  session.page.on('request', onRequest); session.page.on('response', onResponse);
  const capture = async (final = false) => {
    while (pending.size) await Promise.allSettled([...pending]);
    const errors = [...failures]; let proof;
    try {
      proof = await ctx.observe('admin_retention_browser', () => ctx.fixture('browser').evidence(session.page));
      required(proof.authentication === 'browser_session', 'admin_retention_cookie_session');
      check(Array.isArray(proof.requests), 'admin_retention_receipts');
    } catch (error) {errors.push(error);}
    await ctx.observe('admin_retention_browser_writes', async () => structuredClone(writes));
    try {
      const changes = writes.filter(r => r.path === SETTINGS);
      check(changes.length <= (attempted ? 1 : 0), 'admin_retention_extra_write');
      for (const row of writes) {
        // Operations prefetch can submit a real on-demand usage report. Retain
        // that admission separately; it cannot authorize another settings write.
        check((row.path === SETTINGS && row.method === 'PUT') || (row.path === REPORTS && row.method === 'POST'),
          'admin_retention_unexpected_mutation');
        // A prefetch starts independently of settings navigation. Its response
        // can arrive later; close/drain must resolve it before case acceptance.
        if (!final && row.path === REPORTS && !row.response) continue;
        check(row.response?.request_id?.trim(), 'admin_retention_mutation_unresolved');
        if (row.path === REPORTS) check([200, 202].includes(row.response.status)
          && typeof row.response.body.report_id === 'string' && row.response.body.report_id.trim()
          && row.response.body.status === (row.response.status === 200 ? 'ready' : 'pending'),
        'admin_retention_report_prefetch');
        if (proof) check(proof.requests.filter(r => r.request_id === row.response.request_id && r.method === row.method
          && r.status === row.response.status && r.am === ctx.am && r.serving_am === ctx.am).length === 1,
        'admin_retention_ingress');
      }
      if (saved) check(changes.length === 1 && changes[0].response.request_id === saved.request_id,
        'admin_retention_save_join');
      if (proof) for (const r of proof.requests.filter(r => !['GET', 'HEAD', 'OPTIONS'].includes(r.method)))
        check(writes.some(w => w.response?.request_id === r.request_id), 'admin_retention_uncaptured_mutation');
    } catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_retention_browser_proof');
  };
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    session.page.off('request', onRequest); session.page.off('response', onResponse);
    try {await capture(true);} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_retention_browser_finality');
  })();
  ctx.defer('admin_retention_close', close);
  const field = session.page.getByLabel('Agent log retention (days)', {exact: true});
  return {close, async save(days) {
    await navigateSettings(session); await capture();
    const contract = await field.evaluate(node => ({type: node.type, min: node.min, max: node.max, step: node.step, required: node.required}));
    ctx.assert(equal(contract, {type: 'number', min: '0', max: '4294967295', step: '1', required: true}),
      'admin_retention_number_contract');
    for (const boundary of ['0', '4294967295']) {
      await field.fill(boundary);
      ctx.assert(await field.evaluate(node => node.checkValidity()), 'admin_retention_unsigned_boundary');
    }
    const before = await read(ctx, SETTINGS), desired = structuredClone(before.settings);
    desired.retention.agent_log_days = days; state.own(desired);
    await field.fill(String(days)); attempted = true;
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'PUT' && apiPath(r.url()) === SETTINGS), saveSettings(session),
    ]);
    saved = await ctx.observe('admin_retention_save', async () => ({status: response.status(),
      request_id: await response.headerValue('x-request-id'), body: await response.json()}));
    ctx.assert(saved.status === 200 && saved.request_id?.trim(), 'admin_retention_save_response');
    ctx.assert(equal(response.request().postDataJSON(), {expected_revision: before.revision, settings: desired}),
      'admin_retention_save_body');
    ctx.assert(Number.isSafeInteger(saved.body.revision) && saved.body.revision > before.revision
      && sameSettings(saved.body.settings, desired), 'admin_retention_saved_policy');
    await capture();
    for (const am of [ctx.am, other(ctx.am)]) {
      const peer = await read(ctx, SETTINGS, am);
      ctx.assert(peer.revision === saved.body.revision && equal(peer.settings, saved.body.settings), 'admin_retention_peer_policy');
    }
    return saved.body;
  }, async invalid(input, stored) {
    await field.fill(input);
    const number = input === '' ? null : Number(input);
    state.own({...stored.settings, retention: {...stored.settings.retention, agent_log_days: number}});
    ctx.assert(!(await field.evaluate(node => node.checkValidity())), 'admin_retention_invalid_form');
    await session.page.getByRole('button', {name: 'Save settings', exact: true}).click();
    await capture();
    for (const am of ['a', 'b']) ctx.assert(equal(stored, await read(ctx, SETTINGS, am)), 'admin_retention_invalid_ui_effect');
  }};
}
