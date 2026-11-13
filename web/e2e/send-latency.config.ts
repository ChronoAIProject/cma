import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_SEND_URL ?? 'http://127.0.0.1:4371';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Send scheduling fixtures require a loopback compiled preview.');

export default defineConfig({
  testDir: '.',
  testMatch: 'send-latency.check.ts',
  workers: 1,
  retries: 0,
  maxFailures: 1,
  forbidOnly: true,
  timeout: 20_000,
  reporter: 'list',
  outputDir: process.env.CMA_SEND_OUTPUT ?? 'test-results/send-latency',
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
