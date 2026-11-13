import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

export default defineConfig({
  projects: browserProjects(),
  testDir: '.',
  testMatch: 'image-watchdog.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 60_000,
  reporter: [['list'], ['./image-smoke-reporter.ts']],
  outputDir: process.env.CMA_WEB_IMAGE_EVIDENCE_DIR,
  use: { trace: 'retain-on-failure', screenshot: 'only-on-failure', video: 'off' },
});
