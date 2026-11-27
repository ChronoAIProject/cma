import { randomUUID } from 'node:crypto';
import { check, GateError } from '../lib/checks.mjs';
import { BODY_BYTES } from './authority.mjs';
import { paint, openAgent } from '../timing/lib/flows.mjs';

const apiPath = (path) => /^\/api\/v[12]\/[A-Za-z0-9_/?=&%.,:-]+$/.test(path) || path === '/auth/session';
export class BrowserTransport {
  constructor(context, origin, subject, onStop, beforeRead = async () => {}) {
    Object.assign(this, { context, origin, subject, onStop, beforeRead });
    this.permits = new Set(); this.streams = new Map(); this.observers = new Map();
    this.http = { started: 0, completed: 0, failed: 0, open: 0, streamOpened: 0, streamClosed: 0 };
  }
  async initialize() {
    await this.context.route('**/*', async (route) => {
      const request = route.request(); const url = new URL(request.url());
      const read = ['GET', 'HEAD'].includes(request.method());
      const allowed = url.origin === this.origin && !request.redirectedFrom()
        && (read && (!url.pathname.startsWith('/auth/') || url.pathname === '/auth/session')
          || this.permits.has(`${request.method()} ${url.pathname}`));
      if (!allowed) { this.onStop('browser_request_refused'); return route.abort('blockedbyclient'); }
      if (read && (url.pathname.startsWith('/api/') || url.pathname === '/auth/session')) {
        try { await this.beforeRead(); }
        catch { this.onStop('read_quota_deadline'); return route.abort('blockedbyclient'); }
      }
      return route.fallback();
    });
    await this.context.routeWebSocket('**/*', (socket) => { this.onStop('websocket_refused'); socket.close(); });
    this.context.on('request', () => { this.http.started++; this.http.open++; });
    this.context.on('requestfinished', () => { this.http.completed++; this.http.open--; });
    this.context.on('requestfailed', () => { this.http.failed++; this.http.open--; });
    this.context.on('response', (response) => { if (response.status() >= 400) this.onStop('http_refusal'); });
    this.page = await this.context.newPage();
    await this.page.exposeBinding('__scaleStream', async (_, event) => {
      const stream = this.streams.get(event.id);
      check(stream, 'unexpected_stream_callback');
      if (event.type === 'headers') {
        check(event.status === 200 && event.url === stream.url
          && event.contentType?.startsWith('text/event-stream'), 'invalid_stream_response');
        stream.ready(new Response(stream.pipe.readable, { status: 200 }));
      } else if (event.type === 'chunk') {
        check(event.bytes.length <= 65536, 'stream_chunk_limit');
        await stream.writer.write(Uint8Array.from(event.bytes));
      }
    });
    const navigation = await this.page.goto(`${this.origin}/auth/session`, { timeout: 30000 });
    check(navigation?.status() === 200 && this.page.url() === `${this.origin}/auth/session`, 'session_navigation_failed');
    await this.session();
  }
  async session() {
    const { value } = await this.request('/auth/session');
    check(value?.authenticated === true && value.visitor === false && value.subject === this.subject,
      'session_subject_mismatch');
  }
  async request(path, { method = 'GET', body, key, timeoutMs = 30000, expected = [200] } = {}) {
    check(apiPath(path) && new URL(path, this.origin).origin === this.origin, 'route_refused');
    check(['GET', 'POST', 'DELETE'].includes(method), 'method_refused');
    if (method !== 'GET') check(typeof key === 'string' && key.length >= 8, 'key_required');
    const permit = `${method} ${path.split('?')[0]}`;
    this.permits.add(permit);
    try {
      const result = await this.page.evaluate(async ({ origin, path, method, body, key, timeoutMs, maxBytes }) => {
        if (location.origin !== origin) throw new Error('origin');
        const response = await fetch(path, { method, body, credentials: 'same-origin', redirect: 'error',
          headers: { Accept: 'application/json', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
            ...(key ? { 'Idempotency-Key': key } : {}) }, signal: AbortSignal.timeout(timeoutMs) });
        if (response.url !== new URL(path, origin).href) throw new Error('origin');
        const reader = response.body?.getReader(); const decoder = new TextDecoder('utf-8', { fatal: true });
        let bytes = 0; let text = '';
        try {
          while (reader) {
            const { value, done } = await reader.read();
            if (done) break;
            bytes += value.byteLength;
            if (bytes > maxBytes) throw new Error('size');
            text += decoder.decode(value, { stream: true });
          }
          return { status: response.status, text: text + decoder.decode() };
        } finally { await reader?.cancel(); }
      }, { origin: this.origin, path, method, body, key, timeoutMs, maxBytes: BODY_BYTES });
      check(expected.includes(result.status), 'unexpected_http_status');
      return { status: result.status, value: result.text ? JSON.parse(result.text) : null, bytes: Buffer.byteLength(result.text) };
    } finally { this.permits.delete(permit); }
  }
  async stream(path, signal) {
    check(apiPath(path) && new URL(path, this.origin).origin === this.origin, 'route_refused');
    const id = randomUUID(); const pipe = new TransformStream();
    let ready; let fail;
    const headers = new Promise((resolve, reject) => { ready = resolve; fail = reject; });
    const stream = { id, pipe, writer: pipe.writable.getWriter(), ready, fail, url: new URL(path, this.origin).href };
    this.streams.set(id, stream); this.http.streamOpened++;
    const cancel = () => this.page.evaluate((id) => globalThis.__scaleControllers?.get(id)?.abort(), id).catch(() => {});
    signal.addEventListener('abort', cancel, { once: true });
    stream.done = this.page.evaluate(async ({ id, origin, path }) => {
      if (location.origin !== origin) throw new Error('origin');
      const controller = new AbortController();
      globalThis.__scaleControllers ??= new Map(); globalThis.__scaleControllers.set(id, controller);
      let reader;
      try {
        const response = await fetch(path, { credentials: 'same-origin', redirect: 'error', signal: controller.signal,
          headers: { Accept: 'text/event-stream', Prefer: 'cma-message-bindings, cma-live-observed-at' } });
        await globalThis.__scaleStream({ id, type: 'headers', status: response.status, url: response.url,
          contentType: response.headers.get('content-type') });
        reader = response.body.getReader();
        for (;;) {
          const { value, done } = await reader.read(); if (done) break;
          for (let i = 0; i < value.length; i += 65536)
            await globalThis.__scaleStream({ id, type: 'chunk', bytes: Array.from(value.subarray(i, i + 65536)) });
        }
      } finally { await reader?.cancel(); globalThis.__scaleControllers.delete(id); }
    }, { id, origin: this.origin, path }).then(() => stream.writer.close(), (error) => {
      fail(error); return stream.writer.abort(error);
    }).catch(() => {}).finally(() => {
      this.http.streamClosed++; this.streams.delete(id); signal.removeEventListener('abort', cancel);
    });
    if (signal.aborted) await cancel();
    const timer = setTimeout(() => { fail(new Error('stream_open_deadline')); void cancel(); }, 30000);
    try { return await headers; } finally { clearTimeout(timer); }
  }
  async observeAgent(id, timeoutMs = 30000) {
    const page = await this.context.newPage();
    const timer = setTimeout(() => { void page.close().catch(() => {}); }, timeoutMs);
    // The portal's connection readback is the sole permitted observer POST.
    await page.route('**/auth/connections/check', async (route) => {
      const request = route.request(); let body;
      try { body = request.postDataJSON(); } catch { /* Deny malformed bodies. */ }
      if (new URL(request.url()).origin !== this.origin || request.redirectedFrom() || request.method() !== 'POST'
        || body?.target?.type !== 'agent' || body.target.id !== id || body.active !== undefined)
        return route.abort('blockedbyclient');
      return route.continue();
    });
    try { await openAgent(page, this.origin, id); this.observers.set(id, page); }
    finally { clearTimeout(timer); }
  }
  async visible(id, answer, timeoutMs, signal) {
    const page = this.observers.get(id);
    check(page, 'browser_observer_missing');
    check(Number.isFinite(timeoutMs) && timeoutMs > 0, 'browser_observation_budget');
    const deadline = performance.now() + timeoutMs;
    const row = page.locator('.message--assistant .message__body').filter({ hasText: answer });
    const remaining = () => {
      check(!signal?.aborted, 'browser_observation_stopped');
      const ms = deadline - performance.now();
      check(ms > 0, 'browser_observation_deadline'); return ms;
    };
    let timer; let abort;
    const expired = new Promise((_, reject) => {
      timer = setTimeout(() => reject(new GateError('browser_observation_deadline')), timeoutMs);
      abort = () => reject(new GateError('browser_observation_stopped'));
      signal?.addEventListener('abort', abort, { once: true });
      if (signal?.aborted) abort();
    });
    try {
      await Promise.race([expired, (async () => {
        await row.first().waitFor({ state: 'visible', timeout: remaining() });
        remaining(); check(await row.count() === 1, 'duplicate_visible_answer');
        remaining(); await paint(page); remaining();
      })()]);
    } catch (error) {
      if (this.observers.get(id) === page) this.observers.delete(id);
      // Dispose only the DOM observer; API and primary stream custody remain usable.
      void page.close({ runBeforeUnload: false }).catch(() => {});
      throw error;
    } finally { clearTimeout(timer); signal?.removeEventListener('abort', abort); }
  }
  async close() {
    await this.context.close();
    await Promise.allSettled([...this.streams.values()].map((stream) => stream.done));
  }
}
