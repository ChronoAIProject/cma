// Local-only create measurement. No application response or display is mocked.
import { lookup } from 'node:dns/promises';
import { readFile, mkdir, stat, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { performance } from 'node:perf_hooks';
import { OwnedResources, session, openAgent } from './lib/flows.mjs';
import { poll, safeFailure, stats } from './lib/core.mjs';

export function localOrigin(value) {
  const url = new URL(value);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password
      || url.pathname !== '/' || url.search || url.hash
      || !['127.0.0.1', 'localhost', '[::1]', 'am.cma-scale.test'].includes(url.hostname))
    throw new Error('Explicit loopback origin required');
  return url.origin;
}

// Conservative client observation: the product ready marker, an opaque center
// pixel, and Chrome's visibility/occlusion observation must agree. This is an
// observed upper bound, not the timestamp at which the remote frame arrived.
export async function paintedDesktop() {
  const target = document.querySelector('.agent-desktop__frame[data-ready]');
  const canvas = target?.querySelector('canvas');
  if (!canvas || typeof IntersectionObserverEntry === 'undefined'
      || !('isVisible' in IntersectionObserverEntry.prototype)
      || typeof canvas.checkVisibility !== 'function') return false;
  const surfaceReady = () => {
    if (!canvas.isConnected || !target.hasAttribute('data-ready')
        || canvas.width <= 0 || canvas.height <= 0
        || !canvas.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true,
          contentVisibilityAuto: true })) return false;
    const bounds = canvas.getBoundingClientRect();
    const x = bounds.left + bounds.width / 2;
    const y = bounds.top + bounds.height / 2;
    if (bounds.width <= 0 || bounds.height <= 0 || x < 0 || y < 0
        || x >= innerWidth || y >= innerHeight || document.elementFromPoint(x, y) !== canvas)
      return false;
    try {
      return canvas.getContext('2d')?.getImageData(
        Math.floor(canvas.width / 2), Math.floor(canvas.height / 2), 1, 1,
      ).data[3] === 255;
    } catch { return false; } // An unreadable/tainted canvas cannot prove a frame.
  };
  if (!surfaceReady()) return false;
  // Hit tests alone miss pointer-events:none loading masks. Visibility tracking
  // accounts for those masks and ancestor opacity. Unsupported browsers fail
  // closed; no DOM/CSS is altered to make the observation pass.
  return new Promise(resolve => {
    const finish = visible => { clearTimeout(timer); observer.disconnect(); resolve(visible); };
    const observer = new IntersectionObserver(entries => {
      const entry = entries.find(value => value.target === canvas);
      if (entry) finish(entry.isIntersecting && entry.isVisible && surfaceReady());
    }, { trackVisibility: true, delay: 100 });
    const timer = setTimeout(() => finish(false), 1000);
    observer.observe(canvas);
  });
}

export async function waitForDesktop(page, timeout = 30000) {
  // Await each browser result explicitly: a Promise is truthy in versions of
  // Playwright waitForFunction that do not await the predicate before polling.
  return poll(() => page.evaluate(paintedDesktop), Boolean, { timeout, interval: 20 });
}

export function sampleSummary(rows) {
  return Object.fromEntries(['plain', 'desktop'].map(variant => [variant,
    stats(rows.filter(row => row.variant === variant).map(row => ({ ...row,
      outcome: row.cleanup_failure || row.browser_close_failure ? 'failed' : row.outcome,
    })), 5000),
  ]));
}

async function main() {
  if (!process.env.CMA_LOCAL_CREATE_CONFIG) throw new Error('CMA_LOCAL_CREATE_CONFIG required');
  const config = JSON.parse(await readFile(process.env.CMA_LOCAL_CREATE_CONFIG, 'utf8'));
  if (config.environment !== 'isolated-local' || config.synthetic_identity !== true)
    throw new Error('An isolated local synthetic identity is required');
  const origin = localOrigin(config.origin);
  const host = new URL(origin).hostname.replace(/^\[|\]$/g, '');
  const addresses = await lookup(host, { all: true });
  if (!addresses.length || addresses.some(row => !['127.0.0.1', '::1'].includes(row.address)))
    throw new Error('Origin must resolve only to loopback');
  const count = config.samples_per_variant ?? 1;
  if (!Number.isInteger(count) || count < 1 || count > 20)
    throw new Error('samples_per_variant must be 1..20');
  const authentication = await stat(config.storage_state);
  if (!authentication.isFile() || (authentication.mode & 0o077) !== 0)
    throw new Error('Synthetic storage_state must be a private regular file');
  if (!config.playwright_module || !config.browser_executable || !config.output)
    throw new Error('playwright_module, browser_executable and output are required');
  const output = resolve(config.output);
  // Never overwrite an earlier failed/uncertain create journal.
  await mkdir(output, { mode: 0o700 });
  const { chromium } = await import(pathToFileURL(resolve(config.playwright_module)).href);
  const browser = await chromium.launch({ executablePath: config.browser_executable, headless: true });
  const rows = [];
  let failure;
  try {
    const context = await browser.newContext({ storageState: config.storage_state,
      ignoreHTTPSErrors: origin.startsWith('https:'), viewport: { width: 1440, height: 1000 },
      serviceWorkers: 'block', acceptDownloads: false });
    await context.route('**/*', route => new URL(route.request().url()).origin === origin
      ? route.continue() : route.abort('blockedbyclient'));
    await context.routeWebSocket('**/*', socket => {
      const url = new URL(socket.url());
      if (url.host !== new URL(origin).host || !['ws:', 'wss:'].includes(url.protocol)) socket.close();
      else socket.connectToServer();
    });
    const identity = await session(context, origin);
    for (let index = 0; index < count; index++) {
      for (const desktop of index % 2 ? [true, false] : [false, true]) {
        await session(context, origin, identity.subject);
        const page = await context.newPage();
        const journalPath = `${output}/cleanup-${index}-${desktop ? 'desktop' : 'plain'}.json`;
        const owned = new OwnedResources(context, origin, journalPath);
        const row = { index, variant: desktop ? 'desktop' : 'plain', outcome: 'failed',
          started_at: new Date().toISOString(), phases: {}, operation_observations: [] };
        const start = performance.now();
        const elapsed = () => performance.now() - start;
        const save = owned.save.bind(owned);
        let lastPhase;
        owned.save = async () => {
          const state = owned.record.state;
          if (row.phases[state] === undefined) row.phases[state] = elapsed();
          const phase = `${owned.record.lastOperationStatus}/${owned.record.lastOperationPhase}`;
          if (owned.record.lastOperationStatus && phase !== lastPhase) {
            row.operation_observations.push({ elapsed_ms: elapsed(),
              status: owned.record.lastOperationStatus, phase: owned.record.lastOperationPhase });
            lastPhase = phase;
          }
          await save();
        };
        try {
          await owned.create(desktop);
          row.phases.ready = elapsed();
          const frame = desktop ? (async () => {
            await page.waitForURL(`${origin}/agents/${owned.record.agentId}`,
              { waitUntil: 'domcontentloaded', timeout: 30000 });
            await waitForDesktop(page);
            row.phases.desktop_frame = elapsed();
          })() : Promise.resolve();
          // Attach a handler immediately: navigation/composer failure can win.
          frame.catch(() => undefined);
          const composer = openAgent(page, origin, owned.record.agentId).then(() => {
            row.phases.composer_usable = elapsed();
          });
          await Promise.all([frame, composer]);
          row.elapsed_ms = elapsed();
          row.outcome = 'complete';
        } catch (error) {
          row.elapsed_ms = elapsed();
          row.failure = safeFailure(error);
          failure = row.failure;
        } finally {
          // Stop browser polling/streams before retiring only this create's resources.
          try { await page.close(); }
          catch (error) { row.browser_close_failure = safeFailure(error); failure = row.browser_close_failure; }
          try {
            await session(context, origin, identity.subject);
            await owned.cleanup();
            row.cleanup = owned.record.state;
            if (['prepared', 'refused', 'deleted_verified'].includes(row.cleanup))
              await rm(journalPath, { force: true });
          } catch (error) {
            row.cleanup_failure = safeFailure(error);
            failure = row.cleanup_failure;
          }
          rows.push(row);
          await writeFile(`${output}/results.json`, JSON.stringify({ environment: 'isolated-local',
            source: config.source ?? 'unspecified', samples: rows, summary: sampleSummary(rows),
            comparison: 'NOT_EVALUATED', production_acceptance: 'NOT_EVALUATED',
            limits: ['250ms lifecycle polling; times include journal IO and client scheduling.',
              'Summary acceptance covers only the local absolute target, not A3 no-regression.',
              'Client Ready is an observed upper bound; it does not time the durable commit.',
              'Desktop includes visibility/occlusion observation and composer readiness.',
              'Server phase attribution requires the corresponding local API logs.',
              'Local measurements cannot establish production acceptance.'] }, null, 2) + '\n', { mode: 0o600 });
          console.log(JSON.stringify(row));
        }
        if (failure) return;
      }
    }
  } finally {
    await browser.close();
    if (failure) process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => { console.error('Local create setup failed; no credential/error payload is printed.'); process.exitCode = 1; });
}
