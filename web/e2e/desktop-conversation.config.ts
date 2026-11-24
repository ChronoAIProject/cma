import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
const baseURL = process.env.CMA_DESKTOP_PREVIEW_URL ?? 'http://127.0.0.1:4537';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Desktop layout checks require a loopback preview.');
export default defineConfig({
  testDir: '.',
  testMatch: [
    'desktop-conversation.check.ts',
    'desktop-local-time.check.ts',
    'desktop-reconnect.check.ts',
    'deletion-progress.check.ts',
    'parked-update-deletion.check.ts',
    'agent-stop-deletion.check.ts',
    'agent-setup-exit.check.ts',
    'computer-guidance.check.ts',
    'task-help.check.ts',
  ],
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-desktop-conversation-browser-')),
  webServer: {
    command: `npm run preview -- --host 127.0.0.1 --port ${Number(new URL(baseURL).port) || 80} --strictPort`,
    url: baseURL,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    serviceWorkers: 'block',
    launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH },
  },
});
