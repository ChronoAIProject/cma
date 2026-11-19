import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

const baseURL = process.env.CMA_AUTH_PREVIEW_URL ?? 'http://127.0.0.1:4533';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Auth return checks require a loopback preview.');

export default defineConfig({
  projects: browserProjects(),
  testDir: '.',
  testMatch: 'auth-return-target.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-auth-return-')),
  use: { baseURL, serviceWorkers: 'block', trace: 'off', video: 'off' },
});
