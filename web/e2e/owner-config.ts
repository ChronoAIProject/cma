import { mkdtempSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
process.env.CMA_OWNER_E2E_RUN = randomUUID();
export default defineConfig({
  testDir: '.',
  testMatch: 'owner-check.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 900_000,
  expect: { timeout: 20_000 },
  outputDir: mkdtempSync(join(tmpdir(), 'cma-owner-e2e-raw-')),
  globalSetup: './owner-preflight.ts',
  reporter: [['./owner-reporter.ts']],
  use: {
    browserName: 'chromium',
    viewport: { width: 1440, height: 900 },
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
