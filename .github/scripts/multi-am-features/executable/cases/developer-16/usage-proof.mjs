// Match the report consumed by the visible inventory, not a background warmer.
export const durations = {'1h':3600,'24h':86400,'7d':604800,'30d':2592000};
export function matchesReport(report, wanted) {
  if (report?.status !== 'ready' || report.report_complete !== true || !report.summary
    || report.scope?.admin !== false || report.scope.principal !== wanted.principal
    || !(report.expires_at > Date.now()/1000)) return false;
  const query = report.query;
  if(!query?.filters)return false;
  if (!query || query.filters?.owner != null || query.filters?.runtime_version != null
    || query.filters?.statuses?.length) return false;
  if ((query.filters.sandbox_id ?? null) !== (wanted.sandboxId ?? null)
    || (query.filters.agent_id ?? null) !== (wanted.agentId ?? null)
    || (query.filters.search ?? '') !== (wanted.search ?? '').trim().toLowerCase()) return false;
  if (wanted.range === 'custom') return Number(query.from) === wanted.from && Number(query.to) === wanted.to;
  return Number(query.to)-Number(query.from) === durations[wanted.range];
}
export function selectReport(observations, wanted, since = 0) {

  const reports = observations.filter(r=>r.status===200 && !r.capture_error && (r.sequence??1)>since)
    .map(r=>({response:r,report:r.body?.report ?? r.body}))
    .filter(x=>matchesReport(x.report,wanted));
  const pages = observations.filter(r=>r.status===200 && /\/reports\/[^/]+\/sandboxes$/.test(r.path)
    && Array.isArray(r.body?.items));
  for (const page of [...pages].reverse()) {
    const selected = [...reports].reverse().find(x=>x.report.report_id===page.body.report_id);
    if (selected) return {...selected,page};
  }
  return null;
}
export function refreshIdentity(observations, finalId) {
  const admissions = observations.filter(r=>r.method==='POST' && r.path.endsWith('/metrics/reports') && r.status<300);
  const jobs = new Set(admissions.map(r=>r.body?.report_id));
  for (const response of observations) {
    const refresh = response.body?.refresh;
    if (refresh?.state==='pending' && refresh.report_id) jobs.add(refresh.report_id);
  }
  return !jobs.has(undefined) && jobs.size<=1 && (jobs.size===0 || jobs.has(finalId));
}
