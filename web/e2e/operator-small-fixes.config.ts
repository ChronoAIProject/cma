import { defineConfig } from '@playwright/test';

const baseURL = 'http://127.0.0.1:4399';
export default defineConfig({
  testDir: '.',
  testMatch: ['operator-small-fixes.check.ts', 'sandbox-scope.check.ts'],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: './test-results/operator-small-fixes',
  use: {
    baseURL,
    serviceWorkers: 'block',
    trace: 'off',
    screenshot: 'only-on-failure',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
  webServer: {
    command: 'npm run preview -- --host 127.0.0.1 --port 4399 --strictPort',
    url: baseURL,
    reuseExistingServer: false,
  },
});
