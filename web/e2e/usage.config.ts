import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_USAGE_PREVIEW_URL ?? 'http://127.0.0.1:4351';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Usage browser checks require a loopback preview.');
export default defineConfig({
  testDir: '.',
  testMatch: [
    'account-usage.check.ts',
    'operations-usage.check.ts',
    'usage-settings.check.ts',
    'usage-redesign.check.ts',
    'usage-backoff.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_USAGE_EVIDENCE_DIR ?? mkdtempSync(join(tmpdir(), 'cma-usage-browser-')),
  webServer: {
    command: `npm run preview -- --host 127.0.0.1 --port ${Number(new URL(baseURL).port) || 80} --strictPort`,
    url: baseURL,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
  use: {
    baseURL,
    trace: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
