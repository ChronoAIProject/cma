import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

const preview = new URL(process.env.CMA_DOCUMENTS_SEARCH_PREVIEW_URL ?? 'http://127.0.0.1:4682');
if (
  preview.protocol !== 'http:' ||
  !['127.0.0.1', 'localhost'].includes(preview.hostname) ||
  !preview.port
)
  throw new Error('Artifact search checks require a loopback preview port.');

export default defineConfig({
  projects: browserProjects(),
  testDir: '.',
  testMatch: 'documents-search.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_DOCUMENTS_SEARCH_EVIDENCE_DIR ??
    mkdtempSync(join(tmpdir(), 'cma-documents-search-')),
  webServer: {
    command: `npm run preview -- --host ${preview.hostname} --port ${preview.port} --strictPort`,
    url: preview.origin,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  use: {
    baseURL: preview.origin,
    serviceWorkers: 'block',
    trace: 'off',
    video: 'off',
    screenshot: 'off',
  },
});
