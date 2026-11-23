import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

export default defineConfig({
  testDir: '.',
  testMatch: 'topographic-off.check.ts',
  projects: browserProjects(),
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  use: {
    serviceWorkers: 'block',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
});
