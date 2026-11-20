import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
const baseURL = process.env.CMA_DOCUMENTS_PREVIEW_URL ?? 'http://127.0.0.1:4533';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Document checks require a loopback preview.');
export default defineConfig({
  testDir: '.',
  testMatch: [
    'documents.check.ts',
    'documents-unsaved.check.ts',
    'documents-legacy-history.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 45_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-documents-')),
  webServer: {
    command: `npm run preview -- --host 127.0.0.1 --port ${Number(new URL(baseURL).port) || 80} --strictPort`,
    url: baseURL,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
