import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_OPEN_URL ?? 'http://127.0.0.1:4377';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Agent opening measurements require a compiled loopback preview.');

export default defineConfig({
  testDir: '.',
  testMatch: 'agent-open.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: process.env.CMA_OPEN_OUTPUT ?? '../test-results/agent-open',
  projects: ['chromium', 'firefox', 'webkit'].map((browserName) => ({
    name: browserName,
    use: {
      browserName: browserName as 'chromium' | 'firefox' | 'webkit',
      launchOptions:
        browserName === 'chromium' && process.env.CMA_OPEN_CHROMIUM
          ? { executablePath: process.env.CMA_OPEN_CHROMIUM }
          : undefined,
    },
  })),
  use: {
    baseURL,
    viewport: { width: 1440, height: 1000 },
    serviceWorkers: 'block',
    trace: 'on',
    screenshot: 'only-on-failure',
  },
});
