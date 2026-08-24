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
    // The landing character is a real WebGL surface. CI has no hardware GPU,
    // so Chrome's explicit software backend is the deterministic renderer.
    args: [
      '--no-sandbox',
      '--enable-webgl',
      '--use-angle=swiftshader',
      '--enable-unsafe-swiftshader',
    ],
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
 * `waitUntil` is the caller's because it cannot be one answer. A signed-out
 * page settles, so `networkidle0` is the strongest wait available and worth
 * having. A signed-in one never does: the event stream reconnects as soon as
 * it closes, so there is always a request in flight and the page is never
 * idle by that definition — a check waiting for it waits until it times out.
 */
export async function visit(page, port, waitUntil = 'networkidle0') {
  for (let attempt = 0; ; attempt += 1) {
    try {
      await page.goto(`http://localhost:${String(port)}/`, {
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

/** Let React settle after an interaction. */
export function settle(ms = 700) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Wait until the surface a check is about is actually on screen.
 *
 * `settle` is a fixed sleep, which is the right tool after an interaction on a
 * page that is already up. It is the wrong tool for the first paint of a
 * signed-in surface: the bundle is half a megabyte, the runner it executes on
 * is not the machine this was tuned on, and a sleep that is generous locally
 * is a race on a loaded CI worker.
 *
 * When it loses, the whole check misreports. A surface that has not rendered
 * has no controls and measures zero, so the harness reports the absence of
 * things it never waited for — "no New session control", "the sidebar is 0px"
 * — and a green build fails for a reason that is nothing to do with it. A gate
 * that fails on good builds is worse than no gate, because it teaches everyone
 * reading it to re-run rather than to look.
 *
 * So this waits for a signal instead of a duration, and fails loudly with what
 * it was waiting for when the signal never comes.
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
