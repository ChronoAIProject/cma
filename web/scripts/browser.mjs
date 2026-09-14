/**
 * Driving the built app in a real browser.
 *
 * Shared by every check that has to ask a question `jsdom` cannot answer:
 * where a box actually is, what a custom property actually resolves to,
 * whether something is clipped. Extracted rather than copied, because two
 * copies of "where is Chrome" is two things to fix when a runner changes.
 *
 * Nothing here asserts anything. It opens the built bundle and hands back a
 * page; what to ask of it belongs to the check that opened it.
 */
import { spawn } from 'node:child_process';
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

/**
 * Serve the built bundle and open it in a browser.
 *
 * The preview server takes a moment to bind, so the first navigation is
 * retried rather than raced. Returns everything the caller has to close.
 */
export async function open({ port, viewport = { width: 1440, height: 900 } }) {
  const server = spawn('npx', ['vite', 'preview', '--port', String(port)], { stdio: 'ignore' });
  const browser = await puppeteer.launch({
    executablePath: chrome(),
    headless: 'new',
    args: ['--no-sandbox'],
  });
  const page = await browser.newPage();
  await page.setViewport(viewport);
  return {
    page,
    close: async () => {
      await browser.close();
      server.kill();
    },
  };
}

/**
 * Navigate, retrying while the preview server is still coming up.
 *
 * `waitUntil` remains configurable for checks that deliberately hold a
 * request open; the retained shell itself settles at `networkidle0`.
 */
export async function visit(page, port, waitUntil = 'networkidle0', pathname = '/') {
  const url = new URL(pathname, `http://localhost:${String(port)}/`).href;
  for (let attempt = 0; ; attempt += 1) {
    try {
      await page.goto(url, {
        waitUntil,
        timeout: 15_000,
      });
      return;
    } catch (error) {
      if (attempt >= 5) {
        throw error;
      }
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }
  }
}

/**
 * Wait until the surface a check is about is actually on screen.
 *
 * A fixed sleep is a race on a loaded CI worker. Waiting for a selector gives
 * the check a concrete readiness signal and a useful failure message.
 */
export async function ready(page, selector, { timeout = 20_000, label = selector } = {}) {
  try {
    await page.waitForSelector(selector, { visible: true, timeout });
  } catch {
    throw new Error(
      `the surface never rendered: waited ${String(timeout)}ms for ${label} and it did not appear. ` +
        'Every check after this point would have measured an empty page.',
    );
  }
}
