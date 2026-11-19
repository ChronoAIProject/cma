// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { once } from 'node:events';
import { spawn } from 'node:child_process';
import puppeteer from 'puppeteer-core';
import { open, visit, ready } from './browser.mjs';

vi.mock('node:child_process', async (original) => {
  const actual = await original();
  return { ...actual, spawn: vi.fn(actual.spawn) };
});
vi.mock('puppeteer-core', () => ({ default: { launch: vi.fn() } }));
const roots = [];
const listeners = [];
afterEach(async () => {
  for (const result of vi.mocked(spawn).mock.results) {
    const child = result.value;
    if (child?.pid && child.exitCode === null && child.signalCode === null) {
      const closed = once(child, 'close');
      child.kill('SIGKILL');
      await closed;
    }
  }
  for (const server of listeners.splice(0)) {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
  vi.clearAllMocks();
});
async function artifact() {
  const root = await mkdtemp(join(tmpdir(), 'cma-preview-test-'));
  roots.push(root);
  await mkdir(join(root, 'dist'));
  await writeFile(join(root, 'dist/index.html'), '<div id="root">fixture</div>');
  await writeFile(
    join(root, 'dist/build-info.json'),
    JSON.stringify({ version: 'fixture', revision: 'a'.repeat(40), local: true }),
  );
  return root;
}
async function listener() {
  const server = createServer((_request, response) => response.end('unrelated'));
  listeners.push(server);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, port: server.address().port };
}
it('refuses an occupied port before launching a browser or visiting another listener', async () => {
  const directory = await artifact();
  const { port } = await listener();
  puppeteer.launch.mockResolvedValue({
    newPage: async () => ({ setViewport: async () => {} }),
    close: vi.fn(),
  });
  await expect(open({ port, directory })).rejects.toThrow(/preview.*(port|exited|bind)/i);
  expect(puppeteer.launch).not.toHaveBeenCalled();
});
it('closes its owned preview when browser launch fails', async () => {
  const directory = await artifact();
  const { server, port } = await listener();
  await new Promise((resolve) => server.close(resolve));
  listeners.splice(listeners.indexOf(server), 1);
  puppeteer.launch.mockRejectedValue(new Error('Browser launch failed'));
  await expect(open({ port, directory })).rejects.toThrow('Browser launch failed');
  const child = vi.mocked(spawn).mock.results.at(-1).value;
  expect(child.exitCode !== null || child.signalCode !== null).toBe(true);
});

async function freePort() {
  const { server, port } = await listener();
  await new Promise((resolve) => server.close(resolve));
  listeners.splice(listeners.indexOf(server), 1);
  return port;
}
function browserPage() {
  const callbacks = new Map();
  const page = {
    current: '',
    setViewport: vi.fn(),
    on: (event, callback) => callbacks.set(event, callback),
    goto: async (url) => {
      page.current = url;
      return fetch(url);
    },
    url: () => page.current,
    waitForSelector: vi.fn(),
    evaluate: vi.fn(),
  };
  const browser = { newPage: async () => page, close: vi.fn() };
  puppeteer.launch.mockResolvedValue(browser);
  return { page, browser, callbacks };
}
it('serves the expected local build and visits that exact loopback origin', async () => {
  const directory = await artifact();
  const port = await freePort();
  browserPage();
  const owned = await open({ port, directory });
  try {
    await visit(owned.page, port, 'domcontentloaded', '/agent-manager');
    expect(owned.page.url()).toBe(`http://127.0.0.1:${port}/agent-manager`);
    await expect(visit(owned.page, port + 1)).rejects.toThrow('owned preview port');
    await expect(visit(owned.page, port, undefined, 'https://example.invalid/')).rejects.toThrow(
      'owned preview',
    );
  } finally {
    await owned.close();
  }
  await expect(fetch(`http://127.0.0.1:${port}/`)).rejects.toThrow();
});
it.each(['build-info.json', 'index.html'])(
  'rejects a different served %s and closes its listener',
  async (name) => {
    const directory = await artifact();
    const port = await freePort();
    const path = name === 'index.html' ? '/' : '/build-info.json';
    await writeFile(
      join(directory, 'vite.config.mjs'),
      `export default { plugins: [{ name: 'wrong-artifact', configurePreviewServer(server) { server.middlewares.use((req, res, next) => { if (req.url !== ${JSON.stringify(path)}) return next(); res.setHeader('Content-Type', 'application/json'); res.end('{}'); }); } }] };`,
    );
    browserPage();
    await expect(open({ port, directory })).rejects.toThrow(/does not match dist/);
    expect(puppeteer.launch).not.toHaveBeenCalled();
    await expect(fetch(`http://127.0.0.1:${port}/`)).rejects.toThrow();
  },
);
it('reports an early preview exit without launching a browser or leaving a listener', async () => {
  const directory = await artifact();
  const port = await freePort();
  const actual = await vi.importActual('node:child_process');
  vi.mocked(spawn).mockImplementationOnce(() =>
    actual.spawn(process.execPath, ['-e', 'process.exit(7)'], {
      stdio: ['ignore', 'pipe', 'pipe'],
    }),
  );
  await expect(open({ port, directory })).rejects.toThrow(/preview exited \(7\)/);
  expect(puppeteer.launch).not.toHaveBeenCalled();
  await expect(fetch(`http://127.0.0.1:${port}/`)).rejects.toThrow();
});
it.each([false, true])(
  'keeps bounded structural diagnostics for a populated=%s root',
  async (populated) => {
    const directory = await artifact();
    const port = await freePort();
    const { callbacks } = browserPage();
    const owned = await open({ port, directory });
    try {
      await visit(owned.page, port, 'domcontentloaded', '/agent-manager?token=private-fixture');
      owned.page.waitForSelector.mockRejectedValue(new Error('selector missing'));
      owned.page.evaluate.mockResolvedValue({ present: true, populated });
      for (let i = 0; i < 20; i++)
        callbacks.get('console')({
          type: () => 'error',
          location: () => ({ url: `http://127.0.0.1:${port}/app.js?token=private-fixture` }),
          text: () => 'private console payload',
        });
      callbacks.get('requestfailed')({
        url: () => `http://127.0.0.1:${port}/auth/session?token=private-fixture`,
      });
      callbacks.get('pageerror')(new TypeError('private error payload'));
      const error = await ready(owned.page, '.missing').catch((error) => error);
      expect(error.message).toContain('20000ms');
      expect(error.message).toContain('"path":"/agent-manager"');
      expect(error.message).toContain('"selector":".missing"');
      expect(error.message).toContain(`"populated":${populated}`);
      expect(error.message).toContain('"failedRequest":"/auth/session"');
      expect(error.message).toContain('TypeError');
      expect(error.message).not.toMatch(/private|token=/);
      expect((error.message.match(/"console"/g) ?? []).length).toBe(6);
    } finally {
      await owned.close();
    }
  },
);
