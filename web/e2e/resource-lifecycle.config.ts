import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_LIFECYCLE_PREVIEW_URL ?? 'http://127.0.0.1:4367';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Lifecycle fixture checks require a loopback preview.');
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: 'resource-lifecycle.check.ts',
  workers: 1,
  retries: 0,
  timeout: 30000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-resource-lifecycle-browser-')),
  use: {
    baseURL,
    serviceWorkers: 'block',
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
