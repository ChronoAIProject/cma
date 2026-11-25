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
let address = 'about:blank';
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
  return { state: state === 'live' ? 'ready' : state, context_id: context ?? null, location, address: address ?? null, frame: null,
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
  address = 'about:blank';
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
  if (value === 'about:blank') { address = value; return 'Blank page'; }
  const parsed = new URL(value);
  address = ['http:', 'https:'].includes(parsed.protocol) ? parsed.origin : undefined;
  return ['localhost', '127.0.0.1', '[::1]'].includes(parsed.hostname) ? 'Local page' : 'Web page';
}

async function mainFrame() {
  const { frameTree } = await browser.call('Page.getFrameTree', {}, session);
  return frameTree.frame;
}

function frameAddress(frame) {
  return frame.url + (frame.urlFragment ?? '');
}

async function readFirstHeading(frame) {
  const { executionContextId } = await browser.call('Page.createIsolatedWorld', {
    frameId: frame.id, worldName: 'cma-browser-navigation',
  }, session);
  const evaluation = await browser.call('Runtime.evaluate', {
    expression: `new Promise(resolve => {
      const read = () => {
        const heading = document.querySelector('h1');
        resolve({ address: location.href, heading: heading ? heading.textContent.trim().slice(0, 512) : null });
      };
      if (document.readyState === 'complete') read();
      else window.addEventListener('load', read, { once: true });
    })`,
    contextId: executionContextId, awaitPromise: true, returnByValue: true,
  }, session);
  const current = await mainFrame();
  const value = evaluation.result?.value;
  if (evaluation.exceptionDetails || value?.address !== frameAddress(frame)
      || (value.heading !== null && typeof value.heading !== 'string')
      || current.id !== frame.id || current.loaderId !== frame.loaderId
      || frameAddress(current) !== frameAddress(frame)) {
    throw refused('browser_navigation_unconfirmed');
  }
  return value.heading;
}

const navigationHelp = 'Use an HTTP or HTTPS URL without credentials, at most 8192 characters. To preview a local file, serve it over HTTP on 127.0.0.1 and navigate to that URL.';

async function navigate(value) {
  const url = typeof value === 'string' && value.length <= 8192 ? URL.parse(value) : null;
  if (!url || !['http:', 'https:'].includes(url.protocol) || url.username || url.password) {
    throw refused('browser_navigation_invalid');
  }
  state = 'loading';
  frame = undefined;
  const previous = await mainFrame();
  const result = await browser.call('Page.navigate', { url: url.href }, session);
  if (result.errorText) throw refused('browser_navigation_failed');
  const committed = await mainFrame();
  if (result.isDownload) {
    location = safeLocation(committed.url);
    return { navigated: true, location };
  }
  // Chrome omits the loader on same-document navigation. Confirm that the
  // requested address belongs to the original main document in that case.
  const sameDocument = result.loaderId === undefined;
  if (committed.url === 'about:blank' || committed.unreachableUrl
      || committed.id !== result.frameId || committed.loaderId !== (result.loaderId ?? previous.loaderId)
      || (sameDocument && (committed.id !== previous.id || frameAddress(committed) !== url.href))) {
    throw refused('browser_navigation_unconfirmed');
  }
  const firstHeading = await readFirstHeading(committed);
  location = safeLocation(committed.url);
  return { navigated: true, location, first_heading: firstHeading };
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
  const originatingContext = context;
  try {
    if (request.method !== 'POST' || !['/navigate', '/input'].includes(request.url)) { response.writeHead(404).end(); return; }
    let body = '';
    request.setEncoding('utf8');
    for await (const chunk of request.iterator({ destroyOnReturn: false })) {
      body += chunk;
      if (body.length > 16384) {
        // Flush the refusal before closing, even if the caller is still sending.
        response.setHeader('connection', 'close');
        if (request.url === '/navigate') throw refused('browser_navigation_invalid');
        throw new Error('Browser input too large');
      }
    }
    const value = JSON.parse(body);
    const result = await serialized(() => {
      if (originatingContext !== context) throw refused('browser_identity');
      control.requireAgent();
      if (!browser || browser.closed) throw refused('browser_disconnected');
      return request.url === '/navigate' ? navigate(value.url) : sendInput(browser, session, value.input);
    });
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify(result));
  } catch (error) {
    let code = error?.code;
    if (typeof code !== 'string' || !code.startsWith('browser_')) {
      if (browser?.closed) code = 'browser_disconnected';
      else code = request.url === '/navigate' ? 'browser_navigation_failed' : 'browser_request_failed';
    }
    response.setHeader('content-type', 'application/json');
    if (code === 'browser_navigation_invalid') {
      response.writeHead(400).end(JSON.stringify({ error: code, message: navigationHelp }));
    } else {
      response.writeHead(409).end(JSON.stringify({ error: code }));
    }
  }
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
