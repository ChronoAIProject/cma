import { defineConfig } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const baseURL = process.env.CMA_SUGGESTION_PREVIEW_URL ?? 'http://127.0.0.1:4362';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Suggestion browser proof requires a loopback preview.');

export default defineConfig({
  testDir: '.',
  testMatch: 'suggestions.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: process.env.CMA_SUGGESTION_EVIDENCE ?? mkdtempSync(join(tmpdir(), 'cma-suggestions-')),
  use: {
    baseURL,
    serviceWorkers: 'block',
    hasTouch: true,
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
