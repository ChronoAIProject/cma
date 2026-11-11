import { createRequire } from "node:module";
import path from "node:path";
import readline from "node:readline";
import { Guard, ORIGIN, launchOptions, phases, protect } from "./policy.mjs";
import { Observer, contentFingerprint } from "./observer.mjs";

const lines = readline.createInterface({ input: process.stdin });
const messages = lines[Symbol.asyncIterator]();
const receive = async () => JSON.parse((await messages.next()).value);
const send = (value) => process.stdout.write(JSON.stringify(value) + "\n");
let browser;
let snapshot = () => null;
try {
  const config = await receive();
  if (!["public", "agent"].includes(config.mode)) throw Error();
  const bin = process.env.PATH.split(path.delimiter).find((item) =>
    item.endsWith("node_modules/.bin"),
  );
  const require = createRequire(
    path.join(path.dirname(bin), "acceptance-loader.cjs"),
  );
  const { chromium } = require("playwright");
  const puppeteer = require("puppeteer-core");
  if (
    require("playwright/package.json").version !== "1.58.2" ||
    require("puppeteer-core/package.json").version !== "25.17.0"
  )
    throw Error();
  browser = await puppeteer.launch({
    ...launchOptions,
    executablePath: chromium.executablePath(),
  });
  const context = await browser.createBrowserContext();
  await context.setDownloadBehavior({ policy: "deny" });
  const blocked = {
    WRITE: 0,
    DESTINATION: 0,
    SOCKET: 0,
    BUDGET: 0,
    QUIET: 0,
    STOPPED: 0,
  };
  const guard = new Guard();
  if (config.mode === "agent") {
    if (
      !/^[A-Za-z0-9_-]+$/.test(config.agent) ||
      !/^cma_session=[^;\s]+$/.test(config.cookie)
    )
      throw Error();
    await context.setCookie({
      name: "cma_session",
      value: config.cookie.slice("cma_session=".length),
      domain: "cma.nyxid.ai",
      path: "/",
      httpOnly: true,
      secure: true,
      sameSite: "Lax",
    });
  }
  const page = await context.newPage();
  await protect(page, guard, blocked);
  const observer = new Observer(page, ORIGIN);
  const cases = [];
  const bind = async (phase) => {
    if (!phases.includes(phase)) throw Error();
    send({ event: "bind", phase });
    if ((await receive()).ok !== true) throw Error();
  };
  const wait = async (duration) => {
    const until = performance.now() + duration;
    while (performance.now() < until) {
      if (guard.stopped) throw Error();
      await new Promise((resolve) =>
        setTimeout(resolve, Math.min(500, until - performance.now())),
      );
    }
  };
  const observe = async (phase, duration, action, hidden = false) => {
    await bind(phase);
    const start = observer.begin(phase, hidden);
    if (action) await action();
    const windowStart = observer.now();
    await wait(duration);
    const end = observer.now();
    cases.push({
      phase,
      start_ms: start,
      window_start_ms: windowStart,
      end_ms: end,
    });
    observer.phase = "UNMEASURED";
  };
  let initialized = false;
  let history = null;
  let continuity = null;
  let actualHidden = false;
  let navigationReturn = false;
  let readinessMs = null;
  snapshot = (complete = false) => ({
    ...observer.result(),
    blocked,
    cases,
    initialized,
    actual_hidden: actualHidden,
    navigation_return: navigationReturn,
    readiness_ms: readinessMs,
    history,
    continuity,
    browser_version: browserVersion,
    observation_complete: complete,
  });
  const browserVersion = (await browser.version()).split("/").at(-1);
  if (config.mode === "public") {
    const start = observer.begin("A1");
    await page.goto(ORIGIN, { waitUntil: "domcontentloaded", timeout: 30000 });
    await wait(10000);
    initialized = await page.evaluate(() => {
      const root = document.querySelector("#root");
      return (
        !!root &&
        root.childElementCount > 0 &&
        (root.textContent?.trim().length ?? 0) > 0
      );
    });
    cases.push({
      phase: "A1",
      start_ms: start,
      window_start_ms: start,
      end_ms: observer.now(),
    });
  } else {
    await observe("A2", 30000, async () => {
      const start = observer.now();
      await page.goto(`${ORIGIN}/agents/${config.agent}`, {
        waitUntil: "domcontentloaded",
        timeout: 30000,
      });
      await page.waitForSelector("#agent-message:not(:disabled)", {
        timeout: 30000,
        visible: true,
      });
      await page.waitForSelector("[data-message-key]", {
        timeout: 30000,
        visible: true,
      });
      readinessMs = observer.now() - start;
      history = await contentFingerprint(page);
    });
    await observe("A3", 60000);
    const cover = await context.newPage();
    await protect(cover, guard, blocked);
    await cover.goto("about:blank");
    await cover.bringToFront();
    await page.waitForFunction(() => document.visibilityState === "hidden", {
      timeout: 5000,
    });
    actualHidden = await page.evaluate(
      () => document.visibilityState === "hidden",
    );
    if (!actualHidden) throw Error();
    await observe("A4", 60000, null, true);
    await observe("A5_focus", 30000, async () => {
      for (let count = 0; count < 5; count += 1) {
        await cover.bringToFront();
        await page.waitForFunction(
          () => document.visibilityState === "hidden",
          { timeout: 5000 },
        );
        await page.bringToFront();
        await page.waitForFunction(
          () => document.visibilityState === "visible",
          { timeout: 5000 },
        );
      }
    });
    // Use a real in-app link, then browser Back. No synthetic routing or draft writes.
    const link = await page.$('a[href="/"]:not([target="_blank"])');
    if (link) {
      await observe("A5_return", 30000, async () => {
        await link.click();
        await page.waitForFunction(() => location.pathname === "/");
        await page.goBack({ waitUntil: "domcontentloaded" });
        await page.waitForSelector("#agent-message:not(:disabled)", {
          timeout: 30000,
          visible: true,
        });
        navigationReturn = true;
      });
    }
    continuity = await contentFingerprint(page);
    await cover.close();
  }
  // Leave streams censored at the observation boundary, before browser teardown.
  send({ event: "result", result: snapshot(true) });
} catch {
  const partial = snapshot();
  if (partial) send({ event: "result", result: partial });
  else send({ event: "failure", code: "browser_observation_refused" });
  process.exitCode = 1;
} finally {
  if (browser) await browser.close().catch(() => {});
  lines.close();
}
