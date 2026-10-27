import { defineConfig } from '@playwright/test';
import base from './provenance.config';

const port = Number(process.env.CMA_BOT_BROWSER_PORT ?? '4359');
if (!Number.isInteger(port) || port < 1024 || port > 65535)
  throw new Error('Bot browser verification requires a valid local port.');
const origin = `http://127.0.0.1:${port}`;
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
    command: `npm run preview -- --host 127.0.0.1 --port ${port} --strictPort`,
    url: origin,
    reuseExistingServer: false,
  },
});
