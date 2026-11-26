import test, { after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { chromium } from '@playwright/test';
import { BrowserTransport } from './browser.mjs';
import { prepareWire } from './prepare.mjs';
import { clock } from '../lib/checks.mjs';
import { fakeRun, frames, encodeFrames } from '../test/staging-fixtures.mjs';

const directory = await mkdtemp(join(tmpdir(), 'staging-browser-observation-'));
const parser = join(directory, 'wire.mjs');
await prepareWire(parser);
const wire = await import(pathToFileURL(parser).href);
after(() => rm(directory, { recursive: true, force: true }));

async function observer(use) {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ serviceWorkers: 'block' });
  const origin = 'https://staging.invalid';
  await context.route('**/*', async (route) => {
    const url = new URL(route.request().url()); assert.equal(url.origin, origin);
    if (url.pathname === '/auth/session') return route.fulfill({ json: {
      authenticated: true, visitor: false, subject: 'synthetic-owner' } });
    if (url.pathname.endsWith('/events')) return route.fulfill({ contentType: 'text/event-stream',
      body: encodeFrames(frames('Only reply OK synthetic. Do not call tools.')) });
    if (url.pathname.startsWith('/agents/')) return route.fulfill({ contentType: 'text/html', body:
      '<div class="agent-surface__conversation"><input aria-label="Message"><button aria-label="Send message">Send</button>'
      + '<article class="message--assistant"><div class="message__body">OK synthetic</div></article></div>' });
    return route.fulfill({ json: { transportStillUsable: true } });
  });
  const transport = new BrowserTransport(context, origin, 'synthetic-owner', () => {});
  try { await transport.initialize(); await use(transport); }
  finally { await transport.close(); await browser.close(); }
}

async function bounded(promise) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('test detected unbounded observation')), 2000);
  })]); } finally { clearTimeout(timer); }
}

for (const mode of ['suppressed', 'slow']) test(`${mode} animation frames obey the whole observation deadline`, async () => {
  await observer(async (transport) => {
    await transport.observeAgent('agt_synthetic');
    const page = transport.observers.get('agt_synthetic');
    await page.evaluate((mode) => {
      globalThis.requestAnimationFrame = mode === 'suppressed' ? () => 0
        : (callback) => setTimeout(() => callback(performance.now()), 150);
    }, mode);
    const start = performance.now();
    await assert.rejects(bounded(transport.visible('agt_synthetic', 'OK synthetic', 100)), /browser_observation_deadline/);
    assert.ok(performance.now() - start < 1500);
    assert.equal(transport.observers.has('agt_synthetic'), false);
    assert.equal((await transport.request('/api/v1/synthetic')).value.transportStillUsable, true);
    const controller = new AbortController();
    const response = await transport.stream('/api/v2/agents/agt_synthetic/events', controller.signal);
    assert.match(await response.text(), /turn_completed/); controller.abort();
    assert.equal(transport.page.isClosed(), false);
  });
});

for (const mode of ['timeout', 'operator_stop', 'scope_exit']) test(`${mode} releases a hung observer and reaches settlement and exact cleanup`, async () => {
  await observer(async (transport) => {
    const { run, http, rows } = fakeRun(wire); run.time = clock;
    let observing;
    const ready = new Promise((resolve) => { observing = resolve; });
    http.observeAgent = async (id) => {
      await transport.observeAgent(id);
      await transport.observers.get(id).evaluate(() => { globalThis.requestAnimationFrame = () => 0; });
    };
    http.visible = async (id, answer, _timeout, signal) => {
      await transport.observers.get(id).locator('.message__body').evaluate((row, answer) => { row.textContent = answer; }, answer);
      const pending = transport.visible(id, answer, mode === 'timeout' ? 100 : 10000, signal);
      observing(); return pending;
    };
    let turnResult;
    const result = await bounded(run.scope(async (driver) => {
      const fixture = await driver.create(1);
      const pending = fixture.turn('synthetic-offer');
      turnResult = pending.then(() => null, (error) => error);
      await ready;
      if (mode === 'scope_exit') return;
      if (mode === 'operator_stop') driver.stop();
      await turnResult;
    }));
    assert.match((await turnResult)?.message ?? '', /browser_observation_(deadline|stopped)/);
    assert.equal(result.completed, 0); assert.equal(result.settledTurns, 1);
    assert.equal(result.unresolved, 0); assert.equal(result.logicalCleanup, true); assert.equal(result.cleanup, false);
    assert.equal(rows.some((r) => r.scenario === 'rest_turn' && r.status === 'PASS'), false);
    assert.equal(http.calls.filter((c) => c.path.endsWith('/input')).length, 1);
    assert.ok(http.calls.some((c) => c.method === 'DELETE' && c.path === '/api/v1/sandboxes/sbx_synthetic'));
    assert.equal(http.http.streamClosed, 1); assert.equal(http.closed, true);
    assert.equal((await transport.request('/api/v1/synthetic')).value.transportStillUsable, true);
  });
});
