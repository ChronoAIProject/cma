import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_CREDENTIAL_PREVIEW_URL ?? 'http://127.0.0.1:4342';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Credential recovery browser checks require a loopback preview.');
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: [
    'credential-setup.check.ts',
    'public-entry-lifecycle.check.ts',
    'public-entry-reload.check.ts',
    'public-entry-multiroute.check.ts',
    'public-entry-wire.check.ts',
    'public-entry-popup.check.ts',
    'public-entry-identity.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-credential-setup-browser-')),
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
