import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = 'http://127.0.0.1:4664';
export default defineConfig({
  testDir: '.',
  testMatch: 'task-home-repository-failure.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: process.env.CMA_REPOSITORY_EVIDENCE ?? mkdtempSync(join(tmpdir(), 'cma-repository-')),
  webServer: {
    command: 'npm run dev -- --host 127.0.0.1 --port 4664 --strictPort',
    url: baseURL,
    reuseExistingServer: false,
  },
  use: {
    baseURL,
    serviceWorkers: 'block',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
