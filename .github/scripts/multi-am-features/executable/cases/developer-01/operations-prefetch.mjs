import {apiPath, check, required, P} from './common.mjs';

const REPORTS = P + '/usage/metrics/reports';
const READS = new Set(['GET', 'HEAD', 'OPTIONS']);

// Operations warms an admin report even on the settings tab. Keep its exact
// admission evidence; it cannot consume a case's settings/lifecycle write slot.
export function operationsPrefetch(ctx, session) {
  const {page} = session, rows = [], requests = new WeakMap(), pending = new Set(), failures = [];
  required(typeof page.on === 'function' && typeof page.off === 'function', 'admin_operations_capture_binding');
  const onRequest = request => {
    try {
      if (READS.has(request.method()) || apiPath(request.url()) !== REPORTS) return;
      check(rows.length < 256, 'admin_operations_report_bound');
      const row = {method: request.method(), path: REPORTS, body: request.postDataJSON()};
      rows.push(row); requests.set(request, row);
    } catch (error) {failures.push(error);}
  };
  const onResponse = response => {
    const row = requests.get(response.request()); if (!row) return;
    const job = (async () => {
      const body = await response.json();
      row.response = {status: response.status(), request_id: await response.headerValue('x-request-id'),
        report_id: body.report_id, state: body.status};
    })();
    pending.add(job);
    void job.then(() => pending.delete(job), error => {failures.push(error); pending.delete(job);});
  };
  page.on('request', onRequest); page.on('response', onResponse);
  return {async capture(label, final = false) {
    // The caller closes/drains first. Remove listeners even if evidence fails.
    if (final) {page.off('request', onRequest); page.off('response', onResponse);}
    const errors = []; let proof;
    try {proof = await ctx.observe(label, () => ctx.fixture('browser').evidence(page));}
    catch (error) {errors.push(error);}
    // evidence() can itself await a request whose response handler adds a job.
    while (pending.size) await Promise.allSettled([...pending]);
    errors.push(...failures);
    try {await ctx.observe('admin_operations_report_admissions', async () => structuredClone(rows));}
    catch (error) {errors.push(error);}
    const reportIds = new Set();
    try {
      check(Array.isArray(proof?.requests), 'admin_operations_browser_receipts');
      check(proof.requests.every(r => r.request_id && ['a', 'b'].includes(r.serving_am)),
        'admin_operations_attribution');
      for (const row of rows) {
        check(row.method === 'POST', 'admin_operations_report_method');
        if (!final && !row.response) continue;
        const r = row.response;
        check(r && typeof r.request_id === 'string' && r.request_id.trim(), 'admin_operations_report_unresolved');
        check((r.status === 200 && r.state === 'ready' || r.status === 202 && r.state === 'pending')
          && typeof r.report_id === 'string' && r.report_id.trim(), 'admin_operations_report_admission');
        check(!reportIds.has(r.request_id), 'admin_operations_report_duplicate');
        const matched = proof.requests.filter(p => p.request_id === r.request_id);
        check(matched.length === 1 && matched[0].method === 'POST' && matched[0].status === r.status
          && ['a', 'b'].includes(matched[0].am) && matched[0].am === matched[0].serving_am,
        'admin_operations_report_ingress');
        reportIds.add(r.request_id);
      }
    } catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_operations_report_proof');
    // Raw proof above retains every serving field and every report request in
    // aggregate evidence. Only the caller's feature-mutation guard is filtered.
    return {...proof, requests: proof.requests.filter(r => !reportIds.has(r.request_id))};
  }};
}
