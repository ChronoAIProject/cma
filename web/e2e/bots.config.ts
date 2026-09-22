import { defineConfig } from '@playwright/test';
import base from './provenance.config';

const origin = 'http://127.0.0.1:4359';
const fixture = process.env.CMA_BOT_FIXTURE_ROOT;
if (fixture === undefined || !['127.0.0.1', 'localhost'].includes(new URL(fixture).hostname))
  throw new Error('Bot browser verification requires its local CMA fixture.');
export default defineConfig({
  ...base,
  testMatch: 'bots.check.ts',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  use: { ...base.use, baseURL: origin, actionTimeout: 15_000, screenshot: 'only-on-failure' },
  webServer: {
    command: 'npm run preview -- --host 127.0.0.1 --port 4359 --strictPort',
    url: origin,
    reuseExistingServer: false,
  },
});
