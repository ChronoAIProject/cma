import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

const baseURL = process.env.CMA_USAGE_PREVIEW_URL ?? 'http://127.0.0.1:4351';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Usage browser checks require a loopback preview.');
export default defineConfig({
  projects: browserProjects(),
  testDir: '.',
  testMatch: [
    'account-usage.check.ts',
    'operations-usage.check.ts',
    'binding-holds.check.ts',
    'usage-settings.check.ts',
    'usage-redesign.check.ts',
    'usage-coverage.check.ts',
    'usage-evidence.check.ts',
    'usage-backoff.check.ts',
    'usage-ranges.check.ts',
    'usage-initial.check.ts',
    'spend-controls.check.ts',
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
  },
});
