import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

const preview = new URL(process.env.CMA_NEW_AGENT_REQUEST_PREVIEW_URL ?? 'http://127.0.0.1:4692');
if (
  preview.protocol !== 'http:' ||
  !['127.0.0.1', 'localhost'].includes(preview.hostname) ||
  !preview.port ||
  preview.username ||
  preview.password ||
  preview.pathname !== '/' ||
  preview.search ||
  preview.hash
)
  throw new Error('New Agent request checks require a loopback preview port.');

export default defineConfig({
  projects: browserProjects(),
  testDir: '.',
  testMatch: [
    'new-agent-request.check.ts',
    'task-draft-ownership.check.ts',
    'task-templates.check.ts',
    'public-journeys.check.ts',
    'home-simplification.check.ts',
    'preparation-simplification.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_NEW_AGENT_REQUEST_EVIDENCE_DIR ??
    mkdtempSync(join(tmpdir(), 'cma-new-agent-request-')),
  webServer: {
    command: `npm run preview -- --host ${preview.hostname} --port ${preview.port} --strictPort`,
    url: preview.origin,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  use: {
    baseURL: preview.origin,
    serviceWorkers: 'block',
    trace: 'retain-on-failure',
    video: 'off',
    screenshot: 'only-on-failure',
  },
});
