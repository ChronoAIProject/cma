import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: 'deliverable-preview.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 45_000,
  reporter: 'list',
  outputDir: '/Users/liuyitian/code/cma-ui-worktrees/evidence/ticket-08/playwright',
  use: {
    baseURL: process.env.CMA_PREVIEW_URL,
    serviceWorkers: 'block',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'off',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
