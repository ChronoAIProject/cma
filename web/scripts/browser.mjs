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
    args: ['--no-sandbox', '--disable-gpu'],
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

/** Navigate, retrying while the preview server is still coming up. */
export async function visit(page, port) {
  for (let attempt = 0; ; attempt += 1) {
    try {
      await page.goto(`http://localhost:${String(port)}/`, {
        waitUntil: 'networkidle0',
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

/** Let React settle after an interaction. */
export function settle(ms = 700) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Report what is clipping an element, if anything is.
 *
 * The question every one of these checks turns out to need: an element can be
 * present, measurable and entirely invisible because an ancestor scrolls. A
 * fixed element escapes that, which is exactly why a menu portals.
 */
export const CLIPPED_BY = `(element) => {
  const box = element.getBoundingClientRect();
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    const style = getComputedStyle(parent);
    if (style.overflow === 'visible') continue;
    if (style.position === 'static' && getComputedStyle(element).position === 'fixed') continue;
    const clip = parent.getBoundingClientRect();
    if (box.bottom > clip.bottom + 1 || box.top < clip.top - 1 ||
        box.right > clip.right + 1 || box.left < clip.left - 1) {
      return { by: parent.className || parent.tagName, overflow: style.overflow };
    }
  }
  return null;
}`;
