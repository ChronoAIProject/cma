import {apiPath, check, P, required} from './common.mjs';

// Fail one browser transport read, then let the real UI retry through the
// parent's proxy. The failed attempt has no serving AM or synthetic HTTP status.
export async function failSettingsRead(ctx, {session, path}) {
  required(path === P + '/settings', 'admin_read_fault_settings_path_required');
  const page = session?.page;
  required(page && ['route', 'unroute', 'on', 'off'].every(name => typeof page[name] === 'function'),
    'admin_read_fault_browser_binding_missing');
  let origin;
  try {origin = new URL(session.origin);} catch {required(false, 'admin_read_fault_local_origin_required');}
  required(['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname)
    && ['http:', 'https:'].includes(origin.protocol) && !origin.username && !origin.password
    && origin.pathname === '/' && !origin.search && !origin.hash, 'admin_read_fault_local_origin_required');
  const matches = url => url.origin === origin.origin && apiPath(url.href) === path && !url.search;
  const requests = [], indices = new WeakMap(), starts = new WeakMap(), pending = new Set(), errors = [];
  let failedIndex, failedReads = 0, cleanupJob;
  const onRequest = request => {
    if (!matches(new URL(request.url()))) return;
    indices.set(request, requests.length); starts.set(request, performance.now());
    requests.push({method: request.method(), path, status: null,
      started_at: new Date().toISOString(), ended_at: null, duration_ms: null});
  };
  const onResponse = response => {
    const request = response.request(), index = indices.get(request);
    if (index === undefined) return;
    requests[index].status = response.status();
    requests[index].ended_at = new Date().toISOString();
    requests[index].duration_ms = performance.now() - starts.get(request);
  };
  const handle = async route => {
    const request = route.request();
    if (request.method() !== 'GET' || failedIndex !== undefined) {await route.fallback(); return;}
    failedIndex = indices.get(request);
    check(failedIndex !== undefined, 'admin_read_fault_request_unobserved');
    await route.abort('failed'); failedReads++;
    Object.assign(requests[failedIndex], {failure: 'injected_browser_transport_failure',
      serving_am: null, ended_at: new Date().toISOString(),
      duration_ms: performance.now() - starts.get(request)});
  };
  const handler = route => {
    const work = handle(route).catch(error => {errors.push(error);});
    pending.add(work);
    return work.finally(() => pending.delete(work));
  };
  const drain = async () => {while (pending.size) await Promise.all([...pending]);};
  const cleanup = () => cleanupJob ??= (async () => {
    try {await page.unroute(matches, handler);} catch (error) {errors.push(error);}
    await drain();
    for (const [event, listener] of [['request', onRequest], ['response', onResponse]]) {
      try {page.off(event, listener);} catch (error) {errors.push(error);}
    }
    if (errors.length) throw new AggregateError(errors, 'admin_read_fault_cleanup_failed');
  })();
  ctx.defer('admin_settings_read_transport_cleanup', cleanup);
  page.on('request', onRequest); page.on('response', onResponse);
  await page.route(matches, handler);
  return {cleanup, async receipt() {
    await drain();
    return {failed_reads: failedReads, failed_request_index: failedIndex ?? null,
      requests: requests.map(value => ({...value})), error_count: errors.length};
  }};
}
