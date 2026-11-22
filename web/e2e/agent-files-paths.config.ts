import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_FILES_PREVIEW_URL ?? 'http://127.0.0.1:4533';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Local Files path checks require a loopback preview.');

export default defineConfig({
  testDir: '.',
  testMatch: 'agent-files-paths.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 45_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-agent-files-paths-')),
  use: {
    baseURL,
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
