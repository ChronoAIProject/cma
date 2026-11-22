import test from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { BrowserTransport } from './browser.mjs';
import { frames, encodeFrames } from '../test/staging-fixtures.mjs';

test('real browser cookie fetch, stream bridge, rendering and redirect guards use only intercepted synthetic routes', async () => {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ serviceWorkers: 'block' });
  const origin = 'https://staging.invalid'; const hits = []; const stopped = [];
  await context.route('**/*', async (route) => {
    const request = route.request(); const url = new URL(request.url());
    assert.equal(url.origin, origin); hits.push(url.pathname);
    if (url.pathname === '/auth/session') return route.fulfill({ json: { authenticated: true, visitor: false, subject: 'synthetic-owner' } });
    if (url.pathname === '/api/v1/redirect') return route.fulfill({ status: 307, headers: { location: 'https://foreign.invalid/leak' } });
    if (url.pathname === '/api/v1/large') return route.fulfill({ body: 'x'.repeat(4 * 1024 * 1024 + 1) });
    if (url.pathname.endsWith('/input')) {
      assert.equal(request.method(), 'POST'); assert.equal(request.headers()['idempotency-key'], 'synthetic-key');
      return route.fulfill({ status: 202, json: { queued: true } });
    }
    if (url.pathname.endsWith('/events')) return route.fulfill({ contentType: 'text/event-stream',
      body: encodeFrames(frames('Only reply OK synthetic-nonce. Do not call tools.')) });
    if (url.pathname === '/agents/agt_synthetic') return route.fulfill({ contentType: 'text/html', body:
      '<div class="agent-surface__conversation"><input aria-label="Message"><button aria-label="Send message">Send</button>'
      + '<article class="message--user"><div class="message__body">OK synthetic-nonce</div></article></div>' });
    return route.fulfill({ json: {} });
  });
  const transport = new BrowserTransport(context, origin, 'synthetic-owner', (code) => stopped.push(code));
  try {
    await transport.initialize();
    assert.deepEqual((await transport.request('/api/v1/agents/agt_synthetic/input', { method: 'POST', key: 'synthetic-key',
      body: JSON.stringify({ text: 'Only reply OK synthetic-nonce. Do not call tools.' }), expected: [202] })).value, { queued: true });
    await assert.rejects(transport.request('/api/v1/redirect'));
    await assert.rejects(transport.request('/api/v1/large'));
    const controller = new AbortController();
    const response = await transport.stream('/api/v2/agents/agt_synthetic/events', controller.signal);
    const body = await response.text(); controller.abort();
    assert.match(body, /cma.agent.live/); assert.equal(transport.http.streamOpened, 1);
    await transport.observeAgent('agt_synthetic');
    await assert.rejects(transport.visible('agt_synthetic', 'OK synthetic-nonce', 50));
    await transport.observeAgent('agt_synthetic');
    await transport.observers.get('agt_synthetic').evaluate(() => {
      const node = document.createElement('article'); node.className = 'message--assistant';
      node.innerHTML = '<div class="message__body">OK synthetic-nonce</div>'; document.body.append(node);
    });
    await transport.visible('agt_synthetic', 'OK synthetic-nonce', 1000);
    await assert.rejects(transport.page.goto('https://foreign.invalid/leak'));
    assert.ok(stopped.includes('browser_request_refused'));
    assert.equal(hits.includes('/leak'), false);
  } finally { await context.close(); await browser.close(); }
});
