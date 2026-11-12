import { defineConfig } from '@playwright/test';
import remote from './playwright.config';

// This separate config is an explicit opt-in; the existing smoke/mutation sets exclude it.
export default defineConfig({
  ...remote,
  timeout: 1_200_000,
  globalSetup: './visitor-abuse-preflight.ts',
  reporter: [['./visitor-abuse-reporter.ts']],
  projects: [
    // Run the time-sensitive project first when both projects are selected.
    { name: 'tc117-utc', testMatch: 'visitor-abuse-utc.pw.ts', retries: 0, repeatEach: 1 },
    { name: 'tc117', testMatch: 'visitor-abuse.pw.ts', retries: 0, repeatEach: 1 },
  ],
});
