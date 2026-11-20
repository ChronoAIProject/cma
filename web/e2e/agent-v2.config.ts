import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from '@playwright/test';

const preview = new URL(process.env.CMA_AGENT_V2_PREVIEW_URL ?? 'http://127.0.0.1:4359');
if (
  preview.protocol !== 'http:' ||
  !['127.0.0.1', 'localhost'].includes(preview.hostname) ||
  preview.username ||
  preview.password ||
  preview.pathname !== '/' ||
  preview.search ||
  preview.hash
)
  throw new Error('Agent transport browser checks require a loopback preview.');
const port = Number(preview.port) || 80;
const baseURL = `http://127.0.0.1:${port}`;
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: [
    'agent-v2.check.ts',
    'agent-v2-cjk.check.ts',
    'agent-v2-queued.check.ts',
    'agent-v2-composer.check.ts',
    'agent-terminal-recovery.check.ts',
    'agent-history-export.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_AGENT_V2_OUTPUT_DIR ?? mkdtempSync(join(tmpdir(), 'cma-agent-v2-browser-')),
  webServer: {
    cwd: fileURLToPath(new URL('..', import.meta.url)),
    command: `node node_modules/vite/bin/vite.js preview --host 127.0.0.1 --port ${port} --strictPort`,
    url: baseURL,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  use: {
    baseURL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
