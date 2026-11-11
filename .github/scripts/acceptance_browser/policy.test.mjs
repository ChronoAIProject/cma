import assert from "node:assert/strict";
import { createRequire } from "node:module";
import http from "node:http";
import path from "node:path";
import test from "node:test";
import {
  Guard,
  ORIGIN,
  category,
  launchOptions,
  protect,
  quiet,
} from "./policy.mjs";
import { Observer } from "./observer.mjs";

test("route evidence never includes identifiers, queries or remote hosts", () => {
  assert.equal(
    category(ORIGIN + "/api/v2/agents/fixture/events?after=private"),
    "STREAM",
  );
  assert.equal(category(ORIGIN + "/auth/login?return_to=/"), "OTHER");
  assert.equal(category("https://example.test/api/v1/agents/fixture"), "OTHER");
  assert.equal(category(ORIGIN + "/unknown/private-value"), "OTHER");
  assert.equal(category(ORIGIN + "/api/v1/agents/private%2Fvalue"), "OTHER");
});

test("quiet hour and budgets refuse without delaying admitted requests", () => {
  assert.equal(quiet(new Date("2026-11-11T18:00:00Z")), true);
  assert.equal(quiet(new Date("2026-11-11T18:59:59Z")), true);
  assert.equal(quiet(new Date("2026-11-11T19:00:00Z")), false);
  const time = Date.parse("2026-11-11T20:00:00Z");
  const guard = new Guard(ORIGIN, 1);
  assert.equal(guard.admit(ORIGIN, "POST", time), "WRITE");
  assert.equal(guard.admit(ORIGIN, "GET", time), "ALLOW");
  assert.equal(guard.admit(ORIGIN, "GET", time), "BUDGET");
  assert.equal(guard.admit(ORIGIN, "GET", time + 2000), "STOPPED");
});

test("isolated browser blocks mutations, sockets and foreign traffic; measures real visibility", async () => {
  const bin = process.env.PATH.split(path.delimiter).find((entry) =>
    entry.endsWith("node_modules/.bin"),
  );
  const require = createRequire(
    path.join(path.dirname(bin), "acceptance-test.cjs"),
  );
  const { chromium } = require("playwright");
  const counts = { writes: 0, sockets: 0, foreign: 0, reads: 0 };
  const connections = new Set();
  const server = http.createServer((request, response) => {
    if (request.method !== "GET") counts.writes += 1;
    if (request.url === "/assets/local-worker.js") {
      response.writeHead(200, { "Content-Type": "text/javascript" });
      response.end(
        'fetch("/api/v1/agents/fixture",{method:"POST"}).catch(()=>{}).finally(()=>postMessage(true))',
      );
    } else if (request.url.startsWith("/api/v1/agents/fixture")) {
      counts.reads += 1;
      setTimeout(() => {
        response.writeHead(200);
        response.end("{}");
      }, 150);
    } else if (request.url.startsWith("/api/v2/agents/fixture/events")) {
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      response.write("data: {}\n\n");
    } else {
      response.writeHead(200, { "Content-Type": "text/html" });
      response.end("<main>Local fixture</main>");
    }
  });
  server.on("connection", (socket) => {
    connections.add(socket);
    socket.on("close", () => connections.delete(socket));
  });
  server.on("upgrade", (_request, socket) => {
    counts.sockets += 1;
    socket.destroy();
  });
  const foreign = http.createServer((_request, response) => {
    counts.foreign += 1;
    response.end("");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  await new Promise((resolve) => foreign.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const foreignOrigin = `http://127.0.0.1:${foreign.address().port}`;
  const browser = await require("puppeteer-core").launch({
    ...launchOptions,
    executablePath: chromium.executablePath(),
  });
  try {
    const context = await browser.createBrowserContext();
    const blocked = {
      WRITE: 0,
      DESTINATION: 0,
      SOCKET: 0,
      BUDGET: 0,
      QUIET: 0,
      STOPPED: 0,
    };
    // Local clock pin is solely a guard unit-test input, never a production option.
    const guard = new Guard(origin);
    const admit = guard.admit.bind(guard);
    guard.admit = (url, method) =>
      admit(url, method, Date.parse("2026-11-11T20:00:00Z"));
    const page = await context.newPage();
    await protect(page, guard, blocked);
    const observed = new Observer(page, origin);
    await page.goto(origin);
    await page.evaluate(async (other) => {
      const read = "/api/v1/agents/fixture?private=never-recorded";
      await Promise.all([
        fetch(read),
        fetch(read),
        fetch(read, { method: "POST", body: "local fixture" }).catch(() => {}),
        fetch(other).catch(() => {}),
      ]);
      try {
        new WebSocket(location.origin.replace("http", "ws") + "/socket");
      } catch {}
      await new Promise((resolve) => {
        const worker = new Worker("/assets/local-worker.js");
        worker.onmessage = () => {
          worker.terminate();
          resolve();
        };
      });
      window.fixtureStream = new EventSource("/api/v2/agents/fixture/events");
      const controller = new AbortController();
      void fetch(read, { signal: controller.signal }).catch(() => {});
      setTimeout(() => controller.abort(), 30);
    }, foreignOrigin);
    await new Promise((resolve) => setTimeout(resolve, 300));
    const cover = await context.newPage();
    await protect(cover, guard, blocked);
    await cover.goto("about:blank");
    await cover.bringToFront();
    await page.waitForFunction(() => document.visibilityState === "hidden", {
      timeout: 5000,
    });
    assert.equal(await page.evaluate(() => document.visibilityState), "hidden");
    await page.bringToFront();
    assert.equal(
      await page.evaluate(() => document.visibilityState),
      "visible",
    );
    assert.equal(counts.writes, 0);
    assert.equal(counts.sockets, 0);
    assert.equal(counts.foreign, 0);
    assert.ok(counts.reads >= 2);
    assert.equal(blocked.WRITE, 2);
    assert.equal(blocked.SOCKET, 1);
    assert.equal(blocked.DESTINATION, 1);
    assert.ok(observed.rows.some((row) => row.duplicate));
    assert.ok(observed.rows.some((row) => row.canceled));
    assert.equal(
      observed.rows.filter((row) => row.sse && row.censored).length,
      1,
    );
    assert.ok(!JSON.stringify(observed.result()).includes("never-recorded"));
    await context.close();
  } finally {
    await browser.close();
    for (const socket of connections) socket.destroy();
    await Promise.all([
      new Promise((resolve) => server.close(resolve)),
      new Promise((resolve) => foreign.close(resolve)),
    ]);
  }
});
