/** Shared browser checks use one owned, verified loopback preview. */
import { startPreview, localPath } from './preview.mjs';
import { existsSync } from 'node:fs';
import puppeteer from 'puppeteer-core';

/** Where Chrome is, on a runner or on a developer's machine. */
export function chrome() {
  const candidates = [
    process.env.CHROME_PATH,
    '/usr/bin/google-chrome',
    '/usr/bin/chromium-browser',
    '/usr/bin/chromium',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  ].filter((path) => typeof path === 'string' && path.length > 0);
  const found = candidates.find((path) => existsSync(path));
  if (found === undefined) {
    throw new Error(
      `no Chrome found. Set CHROME_PATH, or install one of: ${candidates.join(', ')}`,
    );
  }
  return found;
}

const sessions = new WeakMap();

/** Open only our preview, then release both resources even if browser startup fails. */
export async function open({ port, directory, viewport = { width: 1440, height: 900 } }) {
  const preview = await startPreview({ port, directory });
  let browser;
  const close = async () => {
    try {
      await browser?.close();
    } finally {
      await preview.close();
    }
  };
  try {
    browser = await puppeteer.launch({
      executablePath: chrome(),
      headless: 'new',
      args: ['--no-sandbox', '--disable-field-trial-config'],
    });
    const page = await browser.newPage();
    await page.setViewport(viewport);
    const events = [];
    const record = (value) => {
      events.push(value);
      if (events.length > 8) events.shift();
    };
    page.on('console', (message) => {
      if (['error', 'warn'].includes(message.type()))
        record({
          console: message.type(),
          path: localPath(message.location().url, preview.origin),
        });
    });
    page.on('pageerror', (error) =>
      record({ pageError: /^[A-Za-z]{1,32}Error$/.test(error.name) ? error.name : 'Error' }),
    );
    page.on('requestfailed', (request) =>
      record({ failedRequest: localPath(request.url(), preview.origin) }),
    );
    sessions.set(page, { ...preview, events });
    return { page, close };
  } catch (error) {
    await close();
    throw error;
  }
}

/** Startup and identity are already checked; navigation cannot silently use another port. */
export async function visit(page, port, waitUntil = 'networkidle0', pathname = '/') {
  const session = sessions.get(page);
  if (!session || session.origin !== `http://127.0.0.1:${port}`)
    throw new Error('Navigation requires the owned preview port');
  const url = new URL(pathname, session.origin);
  if (url.origin !== session.origin) throw new Error('Navigation must stay on the owned preview');
  session.assertAlive();
  await page.goto(url.href, { waitUntil, timeout: 15_000 });
  session.assertAlive();
}

/** Bounded structural diagnostics contain no console text, query strings or page contents. */
export async function ready(page, selector, { timeout = 20_000, label = selector } = {}) {
  try {
    sessions.get(page)?.assertAlive();
    await page.waitForSelector(selector, { visible: true, timeout });
  } catch {
    const session = sessions.get(page);
    const root = await page
      .evaluate(() => {
        const element = document.querySelector('#root');
        return { present: element !== null, populated: !!element?.hasChildNodes() };
      })
      .catch(() => ({ unreadable: true }));
    const detail = {
      origin: session?.origin,
      path: localPath(page.url(), session?.origin),
      selector,
      root,
      events: session?.events,
      preview: session?.diagnostics(),
    };
    throw new Error(`Readiness failed after ${timeout}ms for ${label}: ${JSON.stringify(detail)}`);
  }
}
