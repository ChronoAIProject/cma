import {apiPath, check, P, required} from './common.mjs';

// The parent browser's local proxy still dispatches and records the real PUT.
// Only its successful acknowledgement is hidden from the page; no retry is made.
export async function withholdSettingsResponse(ctx, {session, method, path}) {
  required(method === 'PUT' && path === P + '/settings', 'admin_fault_settings_put_required');
  const page = session?.page;
  required(page && ['route', 'unroute', 'on', 'off'].every(name => typeof page[name] === 'function'),
    'admin_fault_browser_binding_missing');
  let origin;
  try {origin = new URL(session.origin);} catch {required(false, 'admin_fault_local_origin_required');}
  required(['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname)
    && ['http:', 'https:'].includes(origin.protocol) && !origin.username && !origin.password
    && origin.pathname === '/' && !origin.search && !origin.hash, 'admin_fault_local_origin_required');
  const matches = url => url.origin === origin.origin && apiPath(url.href) === path && !url.search;
  const requests = [], indices = new WeakMap(), pending = new Set(), errors = [];
  let firstIndex, forwarded = 0, responseStatus = null, withheld = false, cleanupJob;
  const listener = request => {
    if (!matches(new URL(request.url()))) return;
    indices.set(request, requests.length);
    requests.push({method: request.method(), path});
  };
  const handle = async route => {
    const request = route.request();
    if (request.method() !== method) {await route.fallback(); return;}
    if (firstIndex !== undefined) {
      // Preserve actual subsequent dispatches so a UI replay is observable.
      forwarded++; await route.fallback(); return;
    }
    firstIndex = indices.get(request);
    check(firstIndex !== undefined, 'admin_fault_request_unobserved');
    forwarded++;
    let response;
    try {
      response = await route.fetch({maxRetries: 0, maxRedirects: 0, timeout: 10000});
      responseStatus = response.status();
      if (responseStatus >= 200 && responseStatus < 300) {
        await route.abort('failed'); withheld = true;
      } else {
        // A rejection/redirect is genuine server evidence, never a lost success.
        await route.fulfill({response});
      }
    } finally {
      if (response) {
        try {await response.dispose();} catch (error) {errors.push(error);}
      }
    }
  };
  const handler = route => {
    const work = handle(route).catch(async error => {
      errors.push(error);
      // An uncertain forward is not retried. Unblock the UI and retain both errors.
      try {await route.abort('failed');} catch (abortError) {errors.push(abortError);}
    });
    pending.add(work);
    return work.finally(() => pending.delete(work));
  };
  const drain = async () => {while (pending.size) await Promise.all([...pending]);};
  const cleanup = () => cleanupJob ??= (async () => {
    try {await page.unroute(matches, handler);} catch (error) {errors.push(error);}
    await drain();
    try {page.off('request', listener);} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_settings_fault_cleanup_failed');
  })();
  // Capture partial registration failures and keep the hook until case cleanup.
  ctx.defer('admin_settings_response_fault_cleanup', cleanup);
  page.on('request', listener);
  await page.route(matches, handler);
  return {cleanup, async receipt() {
    await drain();
    return {forwarded_writes: forwarded, response_status: responseStatus,
      response_withheld: withheld, error_count: errors.length,
      following_requests: firstIndex === undefined ? [] : requests.slice(firstIndex + 1).map(r => ({...r}))};
  }};
}
