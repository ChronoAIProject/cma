import { egBinding, validEgReceipt } from './receipts.mjs';
import { hash, requireInput } from './core.mjs';

export async function createTransport(ctx, parent) {
  requireInput(
    parent?.scope === 'LOCAL_KIND_DISPOSABLE_TWO_AM' && parent.synthetic === true,
    'eg20_local_kind_lease_required',
  );
  for (const key of ['topology', 'authorize'])
    requireInput(typeof parent[key] === 'function', `eg20_${key}_required`);
  const first = await parent.topology();
  const bind = egBinding;
  const a = bind('a', first),
    b = bind('b', first);
  requireInput(
    a.pod_uid !== b.pod_uid && a.instance_id !== b.instance_id && a.origin !== b.origin,
    'eg20_distinct_eg_processes_required',
  );
  return async function request({
    method = 'GET',
    path,
    body,
    key,
    actor = 'owner',
    replica = ctx.am,
    allowed = [200],
    timeout = 30000,
  }) {
    requireInput(
      /^\/(v1\/|mcp$)/.test(path) &&
        !path.includes('://') &&
        !path.includes('\\') &&
        !path.split('/').includes('..'),
      'eg20_eg_path_required',
    );
    requireInput(
      ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD'].includes(method) &&
        ['a', 'b'].includes(replica),
      'eg20_request_contract',
    );
    requireInput(
      Number.isSafeInteger(timeout) && timeout > 0 && timeout <= 120000,
      'eg20_timeout_contract',
    );
    const before = bind(replica, await parent.topology());
    const target = new URL(path, before.origin);
    requireInput(
      target.origin === before.origin && /^\/(v1\/|mcp$)/.test(target.pathname),
      'eg20_normalized_path_required',
    );
    const headers = new Headers(await parent.authorize({ actor, method, path, replica }));
    if (body !== undefined) headers.set('content-type', 'application/json');
    if (key !== undefined) headers.set('idempotency-key', key);
    const timing = { scope: 'eg', replica, serving: before, started_at: new Date().toISOString() };
    let retained;
    try {
      const response = await fetch(target, {
        method,
        headers,
        redirect: 'manual',
        signal: AbortSignal.timeout(timeout),
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      timing.request_id = response.headers.get('x-request-id');
      const chunks = [];
      let size = 0;
      if (response.body)
        for await (const bytes of response.body) {
          size += bytes.length;
          if (size > 2 * 1024 * 1024) throw Error('eg20_response_size');
          chunks.push(bytes);
        }
      const bytes = Buffer.concat(chunks);
      retained = { scope: 'eg', status: response.status, body: null, eg_receipt: timing };
      retained.body = bytes.length ? JSON.parse(bytes.toString('utf8')) : null;
      timing.ended_at = new Date().toISOString();
      timing.serving_after = bind(replica, await parent.topology());
      timing.attribution = { verdict: 'PASS', basis: 'pod_forward_before_after' };
      if (!validEgReceipt(timing))
        throw Object.assign(Error('eg20_serving_unproved'), { attribution_failure: true });
      if (!allowed.includes(response.status)) throw Error('eg20_http_status_' + response.status);
      return retained;
    } catch (error) {
      if (error?.attribution_failure)
        timing.attribution = { verdict: 'UNPROVED', basis: 'pod_forward_before_after' };
      const safe = Error(
        /^eg20_[a-z0-9_]+$/.test(error?.message ?? '') ? error.message : 'eg20_http_unconfirmed',
      );
      safe.response = retained;
      safe.attribution_failure = error?.attribution_failure === true;
      safe.outcome = ['GET', 'HEAD'].includes(method) ? 'unavailable' : 'unknown';
      throw safe;
    } finally {
      try {
        if (validEgReceipt(timing))
          await ctx.observe('eg20_serving_receipt', async () => ({
            schema: 'multi-eg-request-v1',
            receipt: timing,
          }));
        await ctx.observe('eg20_http_serving', async () => ({
          scope: 'eg',
          method,
          path_sha256: hash(path),
          status: retained?.status ?? null,
          ...timing,
          body_sha256: hash(retained?.body ?? null),
        }));
      } catch {
        const error = Error('eg20_private_receipt_write_failed');
        error.response = retained;
        error.outcome = ['GET', 'HEAD'].includes(method) ? 'unavailable' : 'unknown';
        throw error;
      }
    }
  };
}
