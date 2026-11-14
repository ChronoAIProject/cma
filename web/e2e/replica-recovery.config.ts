import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_RECOVERY_URL ?? 'http://127.0.0.1:4372';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Replica recovery fixtures require a loopback compiled preview.');

export default defineConfig({
  testDir: '.',
  testMatch: ['replica-recovery.check.ts', 'auth-recovery.check.ts'],
  workers: 1,
  retries: 0,
  maxFailures: 1,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: process.env.CMA_RECOVERY_OUTPUT ?? 'test-results/replica-recovery',
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
