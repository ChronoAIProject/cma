import { check, clock, hash } from './checks.mjs';
import { guardedFetch, localUrl } from './safety.mjs';

export class LocalHttp {
  constructor(origin, { bearer, fetchImpl, timeoutMs = 30000, time = clock } = {}) {
    const url = localUrl(origin);
    check(url.pathname === '/' && !url.search, 'http_origin_required');
    this.origin = url.origin;
    this.bearer = bearer;
    this.fetch = guardedFetch(fetchImpl);
    this.timeoutMs = timeoutMs;
    this.time = time;
    this.samples = [];
  }

  async request(path, { method = 'GET', body, key, expected = [200], route, signal } = {}) {
    check(typeof path === 'string' && path.startsWith('/') && !path.startsWith('//'), 'relative_route_required');
    const url = new URL(path, this.origin);
    check(url.origin === this.origin, 'origin_changed');
    check(typeof route === 'string' && /^[A-Za-z0-9_/:.-]{1,160}$/.test(route), 'route_template_required');
    check(['GET', 'POST', 'DELETE'].includes(method), 'method_refused');
    if (method !== 'GET') check(typeof key === 'string' && key.length >= 8, 'idempotency_key_required');
    const started = this.time.now();
    let ok = false; let status = null;
    try {
      const response = await this.fetch(url.href, { method,
        headers: { Accept: 'application/json', ...(this.bearer ? { Authorization: `Bearer ${this.bearer}` } : {}),
          ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...(key ? { 'Idempotency-Key': key } : {}) },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(this.timeoutMs)]) : AbortSignal.timeout(this.timeoutMs) });
      status = response.status;
      check(expected.includes(status), 'unexpected_http_status');
      // The timer includes response-body completion, as in timing-harness/lib/core.mjs.
      const chunks = []; let bytes = 0;
      if (response.body) for await (const chunk of response.body) {
        bytes += chunk.byteLength;
        check(bytes <= 4 * 1024 * 1024, 'response_body_limit');
        chunks.push(chunk);
      }
      const text = Buffer.concat(chunks).toString('utf8');
      const value = text ? JSON.parse(text) : null;
      ok = true;
      return { status, value, evidenceHash: hash(text) };
    } finally {
      this.samples.push({ route, elapsedMs: this.time.now() - started, ok, status });
    }
  }

  command(id, options = {}) {
    check(typeof id === 'string' && /^[A-Za-z0-9_-]{1,256}$/.test(id), 'invalid_command_id');
    return this.request(`/api/v2/commands/${id}`, { ...options, route: '/api/v2/commands/:command' });
  }
}
