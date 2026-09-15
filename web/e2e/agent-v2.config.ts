import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_AGENT_V2_PREVIEW_URL ?? 'http://127.0.0.1:4359';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Agent transport browser checks require a loopback preview.');
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: ['agent-v2.check.ts', 'agent-v2-queued.check.ts'],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-agent-v2-browser-')),
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
