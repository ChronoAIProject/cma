import { defineConfig } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const baseURL = process.env.CMA_BROWSER_PREVIEW_URL ?? 'http://127.0.0.1:4315';
if (new URL(baseURL).hostname !== '127.0.0.1') throw new Error('Loopback preview required');
export default defineConfig({
  testDir: '.',
  testMatch: ['browser-observation.check.ts'],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-browser-observation-')),
  use: {
    baseURL,
    serviceWorkers: 'block',
    trace: 'off',
    video: 'off',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
