const { randomUUID } = require('node:crypto');
const { mkdir, rm, chmod } = require('node:fs/promises');
const { createServer } = require('node:http');
const { join } = require('node:path');
const { createInterface } = require('node:readline');
const { DevTools } = require('./devtools.cjs');
const { BrowserControl, refused } = require('./control.cjs');
const { sendInput } = require('./input.cjs');
const { setTimeout: delay } = require('node:timers/promises');

const directory = process.argv[2];
const observers = new Map();
let browser;
let profile;
let session;
let context;
let control = new BrowserControl();
let frame;
let state = 'not_enabled';
let location = 'Blank page';
let sequence = 0;
let captures = 0;
let bytes = 0;
let queue = Promise.resolve();
let queued = 0;
let lastCapture = -Infinity;
const deliveries = [];

function serialized(operation) {
  if (queued >= 32) return Promise.reject(new Error('Browser busy'));
  queued += 1;
  const result = queue.then(operation);
  queue = result.catch(() => {}).finally(() => { queued -= 1; });
  return result;
}

function status() {
  for (const [observer, deadline] of observers) if (deadline <= Date.now()) observers.delete(observer);
  if (browser?.closed) { state = 'disconnected'; frame = undefined; }
  return { state: state === 'live' ? 'ready' : state, context_id: context ?? null, location, frame: null,
    observers: observers.size, captures, bytes, min_interval_ms: 1000, fresh_for_ms: 3000,
    max_frame_bytes: 262144, max_observers: 16 };
}

async function disable() {
  await browser?.stop();
  if (profile) await rm(profile, { recursive: true, force: true, maxRetries: 5 });
  profile = undefined;
  browser = undefined;
  frame = undefined;
  context = undefined;
  control = new BrowserControl();
  session = undefined;
  observers.clear();
  state = 'not_enabled';
  location = 'Blank page';
  return status();
}

async function enable() {
  if (browser && !browser.closed) return status();
  await disable();
  await mkdir(directory, { recursive: true, mode: 0o700 });
  profile = join(directory, 'profile-' + randomUUID());
  browser = new DevTools(profile);
  try {
    await browser.call('Browser.getVersion');
    const created = await browser.call('Target.createBrowserContext');
    const target = await browser.call('Target.createTarget', { url: 'about:blank', browserContextId: created.browserContextId });
    const attached = await browser.call('Target.attachToTarget', { targetId: target.targetId, flatten: true });
    session = attached.sessionId;
    await browser.call('Emulation.setDeviceMetricsOverride', { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false }, session);
    await browser.call('Page.enable', {}, session);
    context = randomUUID();
    state = 'loading';
  } catch {
    await browser.stop();
    browser = undefined;
    state = 'unsupported';
  }
  return status();
}

function safeLocation(value) {
  if (value === 'about:blank') return 'Blank page';
  const parsed = new URL(value);
  return ['localhost', '127.0.0.1', '[::1]'].includes(parsed.hostname) ? 'Local page' : 'Web page';
}

async function navigate(value) {
  control.requireAgent();
  if (!browser || browser.closed) throw new Error('Browser is not enabled');
  const url = new URL(value);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || value.length > 8192)
    throw new Error('Navigation requires an HTTP or HTTPS page without credentials');
  state = 'loading';
  frame = undefined;
  const result = await browser.call('Page.navigate', { url: url.href }, session);
  if (result.errorText) throw new Error('Navigation failed');
  await browser.call('Runtime.evaluate', {
    expression: 'new Promise(resolve => { if (document.readyState === "complete") resolve(true); else window.addEventListener("load", () => resolve(true), {once:true}); })',
    awaitPromise: true, returnByValue: true,
  }, session);
  const tree = await browser.call('Page.getFrameTree', {}, session);
  location = safeLocation(tree.frameTree.frame.url);
  return { navigated: true, location };
}

async function observe(request) {
  const current = status();
  if (!browser || browser.closed) return current;
  if (typeof request.observer !== 'string' || !/^[a-zA-Z0-9-]{1,64}$/.test(request.observer))
    throw new Error('Observer identity required');
  if (request.context_id && request.context_id !== context) throw new Error('Browser context changed');
  if (!observers.has(request.observer) && observers.size >= 16) return { ...current, state: 'paused' };
  observers.set(request.observer, Date.now() + 4000);
  while (deliveries.length && performance.now() - deliveries[0] >= 1000) deliveries.shift();
  if (deliveries.length >= 16) return current;
  if (!frame || Date.now() - Date.parse(frame.captured_at) >= 1000) {
    try {
      const remaining = 1000 - (performance.now() - lastCapture);
      if (remaining > 0) await delay(remaining);
      const screenshot = await browser.call('Page.captureScreenshot', { format: 'jpeg', quality: 45,
        captureBeyondViewport: false, clip: { x: 0, y: 0, width: 1280, height: 720, scale: 1 } }, session);
      lastCapture = performance.now();
      captures += 1;
      const size = Buffer.byteLength(screenshot.data, 'base64');
      if (size > 262144) { state = 'paused'; frame = undefined; return status(); }
      bytes += size;
      frame = { sequence: ++sequence, captured_at: new Date().toISOString(), media_type: 'image/jpeg',
        width: 1280, height: 720, data: screenshot.data };
      state = 'live';
      const tree = await browser.call('Page.getFrameTree', {}, session);
      location = safeLocation(tree.frameTree.frame.url);
    } catch { state = 'disconnected'; frame = undefined; }
  }
  if (frame) deliveries.push(performance.now());
  return { ...status(), state, frame: frame ?? null };
}

async function dispatch(request) {
  switch (request.operation) {
    case 'status': return status();
    case 'enable': return enable();
    case 'disable': return disable();
    case 'observe': return observe(request);
    case 'release': observers.delete(request.observer); return status();
    case 'control':
      if (!browser || browser.closed || !context || (request.context_id && request.context_id !== context)) throw refused('browser_identity');
      if (!request.context_id && request.command !== 'status') throw refused('browser_identity');
      return { ...await control.dispatch(request, input => sendInput(browser, session, input)), context_id: context };
    default: throw new Error('Unsupported browser operation');
  }
}

const agent = createServer(async (request, response) => {
  try {
    if (request.method !== 'POST' || !['/navigate', '/input'].includes(request.url)) { response.writeHead(404).end(); return; }
    let body = '';
    for await (const chunk of request) {
      body += chunk;
      if (body.length > 16384) throw new Error('Navigation too large');
    }
    const value = JSON.parse(body);
    const result = await serialized(() => {
      control.requireAgent();
      if (!browser || browser.closed) throw refused('browser_disconnected');
      return request.url === '/navigate' ? navigate(value.url) : sendInput(browser, session, value.input);
    });
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify(result));
  } catch (error) { response.writeHead(409).end(JSON.stringify({ error: error.code ?? 'browser_disconnected' })); }
});
agent.requestTimeout = 15000;
agent.maxConnections = 8;

async function main() {
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const socket = join(directory, 'browser.sock');
  await rm(socket, { force: true });
  await new Promise(resolve => agent.listen(socket, resolve));
  await chmod(socket, 0o600);
  const input = createInterface({ input: process.stdin });
  input.on('line', async line => {
    let request;
    try {
      if (line.length > 16384) throw new Error('Request too large');
      request = JSON.parse(line);
      const value = await serialized(() => dispatch(request));
      process.stdout.write(JSON.stringify({ id: request.id, value }) + '\n');
    } catch (error) {
      process.stdout.write(JSON.stringify({ id: request?.id, error: error.code ?? 'Browser request refused' }) + '\n');
    }
  });
  const close = async () => { await disable(); agent.close(); process.exit(0); };
  input.once('close', close);
  process.once('SIGTERM', close);
}

main().catch(() => process.exit(1));
