import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: 'operation-replay.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 60_000,
  reporter: 'list',
  outputDir: process.env.CMA_REPLAY_BROWSER_OUTPUT,
  use: {
    baseURL: 'http://127.0.0.1:45818',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
