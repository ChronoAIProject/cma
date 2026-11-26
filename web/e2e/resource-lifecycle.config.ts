import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

const preview = new URL(process.env.CMA_LIFECYCLE_PREVIEW_URL ?? 'http://127.0.0.1:4367');
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
  throw new Error('Lifecycle fixture checks require a loopback preview port.');
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  projects: browserProjects(),
  testDir: '.',
  testMatch: [
    'resource-lifecycle-creation.check.ts',
    'resource-lifecycle-progress.check.ts',
    'resource-lifecycle-shell.check.ts',
    'resource-lifecycle-overview.check.ts',
    'resource-lifecycle-history.check.ts',
    'resource-lifecycle-deletion.check.ts',
    'agent-stop-deletion.check.ts',
    'parked-runtime-status.check.ts',
    'parked-task-deletion.check.ts',
    'home-navigation-simplification.check.ts',
    'retired-agent.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_LIFECYCLE_EVIDENCE_DIR ??
    mkdtempSync(join(tmpdir(), 'cma-resource-lifecycle-browser-')),
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
    screenshot: 'only-on-failure',
    video: 'off',
  },
});
