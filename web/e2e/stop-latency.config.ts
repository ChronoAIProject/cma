import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_STOP_URL ?? 'http://127.0.0.1:4369';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Stop scheduling fixtures require a loopback compiled preview.');

export default defineConfig({
  testDir: '.',
  testMatch: 'stop-latency.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: process.env.CMA_STOP_OUTPUT ?? 'test-results/stop-latency',
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
