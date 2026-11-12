import { defineConfig } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const baseURL = process.env.CMA_UI_PERF_URL ?? 'http://127.0.0.1:4367';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Performance fixtures require a loopback compiled preview.');

export default defineConfig({
  testDir: '.',
  testMatch: ['ui-performance.check.ts', 'agent-polling.check.ts', 'agent-requests.check.ts'],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 120_000,
  reporter: 'list',
  outputDir: process.env.CMA_UI_PERF_OUTPUT ?? mkdtempSync(join(tmpdir(), 'cma-ui-performance-')),
  projects: ['chromium', 'firefox', 'webkit'].map((browserName) => ({
    name: browserName,
    use: { browserName: browserName as 'chromium' | 'firefox' | 'webkit' },
  })),
  use: {
    baseURL,
    actionTimeout: 10_000,
    viewport: { width: 1440, height: 1000 },
    serviceWorkers: 'block',
    trace: 'off',
    screenshot: 'off',
    video: 'off',
  },
});
