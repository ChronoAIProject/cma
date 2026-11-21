import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = 'http://127.0.0.1:4691';
export default defineConfig({
  testDir: '.',
  testMatch: 'sandbox-capacity.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 60_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-sandbox-capacity-')),
  webServer: {
    command: 'npm run preview -- --host 127.0.0.1 --port 4691 --strictPort',
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
