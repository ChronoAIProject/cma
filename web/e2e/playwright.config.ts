import { mkdtempSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

// Raw runner diagnostics are temporary and are never uploaded as staging evidence.
const scratch = mkdtempSync(join(tmpdir(), 'cma-playwright-'));
process.env.CMA_E2E_EVIDENCE_RUN = randomUUID();
// Playwright otherwise writes error-context.md with the current page's raw DOM.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: '**/*.pw.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 600_000,
  expect: { timeout: 20_000 },
  outputDir: scratch,
  globalSetup: './preflight.ts',
  reporter: [['./reporter.ts']],
  use: {
    browserName: 'chromium',
    viewport: { width: 1440, height: 900 },
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
  projects: [
    { name: 'smoke', testMatch: '**/smoke.pw.ts' },
    ...(process.env.CMA_E2E_MODE === 'mutation'
      ? [{ name: 'mutation', testIgnore: ['**/smoke.pw.ts', '**/visitor-abuse*.pw.ts'] }]
      : []),
  ],
});
