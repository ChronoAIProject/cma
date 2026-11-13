import { defineConfig } from '@playwright/test';
import remote from './playwright.config';

export default defineConfig({
  ...remote,
  timeout: 900_000,
  globalSetup: './j0-j2-preflight.ts',
  reporter: [['./j0-j2-reporter.ts']],
  projects: [{ name: 'j0-j2', testMatch: 'new-user.pw.ts', retries: 0, repeatEach: 1 }],
});
