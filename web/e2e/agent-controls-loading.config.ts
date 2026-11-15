import { defineConfig } from '@playwright/test';
import { resolve } from 'node:path';

const baseURL = process.env.CMA_CONTROL_PREVIEW_URL ?? 'http://127.0.0.1:4545';
const outputDir = process.env.CMA_CONTROL_OUTPUT ?? resolve('test-results/agent-controls');
if (!['localhost', '127.0.0.1'].includes(new URL(baseURL).hostname))
  throw new Error('Agent control checks require a local compiled preview.');
export default defineConfig({
  testDir: '.',
  testMatch: 'agent-controls-loading.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 90_000,
  reporter: [['list'], ['json', { outputFile: resolve(outputDir, 'results.json') }]],
  outputDir,
  webServer: {
    command: `npm run preview -- --host 127.0.0.1 --port ${new URL(baseURL).port} --strictPort`,
    url: baseURL,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  projects: ['chromium', 'firefox', 'webkit'].map((browserName) => ({
    name: browserName,
    use: { browserName: browserName as 'chromium' | 'firefox' | 'webkit' },
  })),
  use: {
    baseURL,
    viewport: { width: 1440, height: 1000 },
    serviceWorkers: 'block',
    trace: 'off',
    screenshot: 'off',
    video: 'off',
  },
});
