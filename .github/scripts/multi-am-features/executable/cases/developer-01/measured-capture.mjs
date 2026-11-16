import {apiPath, check, required} from './common.mjs';

export function captureMeasured(ctx, session, actor, am, enabled) {
  const rows = [], pending = new Set(), errors = [];
  const base = '/api/v1/' + (actor === 'admin' ? 'admin/' : '') + 'usage/metrics/reports';
  let count = 0, closeJob;
  const listener = response => {
    const task = (async () => {
      const path = apiPath(response.url());
      if (path !== '/auth/session' && !path.startsWith(base)) return;
      check(++count <= 256, 'admin_measured_capture_bound');
      const raw = await response.json(), method = response.request().method();
      const body = path === '/auth/session' ? {authenticated: raw.authenticated, subject: raw.subject,
        usage_reporting: raw.usage_reporting} : raw;
      rows.push({path, query: new URL(response.url()).search, method, status: response.status(),
        request_id: await response.headerValue('x-request-id'), body,
        ...(method === 'POST' ? {request_body: response.request().postDataJSON()} : {})});
    })();
    pending.add(task);
    void task.then(() => pending.delete(task), error => {errors.push(error); pending.delete(task);});
  };
  session.page.on('response', listener);
  const flush = async () => {
    while (pending.size) await Promise.allSettled([...pending]);
    if (errors.length) throw new AggregateError(errors, 'admin_measured_capture_failed');
    return rows;
  };
  const proof = async () => {
    await flush();
    const capture = await ctx.observe('admin_measured_browser', () => ctx.fixture('browser').evidence(session.page));
    required(capture.authentication === 'browser_session', 'admin_measured_cookie_session');
    check(Array.isArray(capture.requests), 'admin_measured_receipts');
    for (const row of rows) check(typeof row.request_id === 'string' && row.request_id.trim()
      && capture.requests.filter(r => r.request_id === row.request_id && r.method === row.method
        && r.status === row.status && r.am === am && r.serving_am === am).length === 1, 'admin_measured_response_ingress');
    for (const receipt of capture.requests.filter(r => !['GET', 'HEAD', 'OPTIONS'].includes(r.method)))
      check(receipt.method === 'POST' && rows.some(r => r.path === base && r.method === 'POST'
        && r.request_id === receipt.request_id), 'admin_measured_unexpected_write');
    if (!enabled) check(!rows.some(r => r.path.endsWith('/series')), 'admin_measured_hidden_series_request');
    await ctx.observe('admin_measured_responses', async () => ({actor, am, enabled, rows: structuredClone(rows)}));
    return capture;
  };
  const close = () => closeJob ??= (async () => {
    const failures = [];
    try {await session.close();} catch (error) {failures.push(error);}
    session.page.off('response', listener);
    try {await proof();} catch (error) {failures.push(error);}
    if (failures.length) throw new AggregateError(failures, 'admin_measured_browser_finality');
  })();
  ctx.defer('admin_measured_report_close', close);
  return {base, rows, flush, proof, close};
}
