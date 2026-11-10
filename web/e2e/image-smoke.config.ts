import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

const baseURL = process.env.CMA_WEB_IMAGE_URL;
if (!baseURL || !['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Image smoke checks require a loopback Docker server.');

export default defineConfig({
  projects: browserProjects(['--enable-unsafe-swiftshader']),
  testDir: '.',
  testMatch: [
    'image-smoke.check.ts',
    'usage-filters-image.check.ts',
    'owner-chart-image.check.ts',
    'build-identity.check.ts',
    'usage-backoff.check.ts',
    'agent-polling.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 60_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_WEB_IMAGE_EVIDENCE_DIR ?? mkdtempSync(join(tmpdir(), 'cma-web-image-')),
  use: {
    baseURL,
    viewport: { width: 1440, height: 1000 },
    serviceWorkers: 'block',
    trace: 'off',
    video: 'off',
  },
});
