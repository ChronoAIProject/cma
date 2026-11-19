import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

const baseURL = process.env.CMA_AGENT_DRAFT_PREVIEW_URL ?? 'http://127.0.0.1:4533';
if (!['127.0.0.1', 'localhost'].includes(new URL(baseURL).hostname))
  throw new Error('Agent draft checks require a loopback preview.');
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: 'agent-draft-reload.check.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 30_000,
  reporter: 'list',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-agent-draft-reload-')),
  use: { baseURL, trace: 'off', screenshot: 'off', video: 'off', serviceWorkers: 'block' },
});
