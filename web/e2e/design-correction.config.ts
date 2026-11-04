import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: ['design-correction.check.ts'],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 45_000,
  reporter: 'list',
  outputDir: process.env.CMA_DESIGN_EVIDENCE ?? '/tmp/cma-design-correction',
  webServer: {
    command: 'npm run dev -- --host 127.0.0.1 --port 4644 --strictPort',
    url: 'http://127.0.0.1:4644',
    reuseExistingServer: !process.env.CI,
  },
  use: {
    baseURL: 'http://127.0.0.1:4644',
    serviceWorkers: 'block',
    trace: 'off',
    screenshot: 'only-on-failure',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
