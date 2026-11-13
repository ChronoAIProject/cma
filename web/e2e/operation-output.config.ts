import { defineConfig } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export default defineConfig({
  testDir: '.',
  testMatch: 'operation-output.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 60_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_REPLAY_BROWSER_OUTPUT ?? mkdtempSync(join(tmpdir(), 'cma-operation-output-')),
  webServer: {
    command: 'npm run preview -- --host 127.0.0.1 --port 45818 --strictPort',
    url: 'http://127.0.0.1:45818',
    reuseExistingServer: !process.env.CI,
  },
  use: {
    baseURL: 'http://127.0.0.1:45818',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
