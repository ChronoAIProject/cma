import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { browserProjects } from './browser-projects';

const baseURL = 'http://127.0.0.1:4383';

export default defineConfig({
  projects: browserProjects(),
  testDir: '.',
  testMatch: 'gateway-accessibility.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir:
    process.env.CMA_GATEWAY_A11Y_EVIDENCE_DIR ?? mkdtempSync(join(tmpdir(), 'cma-gateway-a11y-')),
  webServer: {
    command: 'npm run preview -- --host 127.0.0.1 --port 4383 --strictPort',
    url: baseURL,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  use: { baseURL, trace: 'off', video: 'off', serviceWorkers: 'block' },
});
