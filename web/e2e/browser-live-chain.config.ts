import { defineConfig } from '@playwright/test';
import observation from './browser-observation.config';

if (!process.env.CMA_BROWSER_RELAY_DIRECTORY)
  throw new Error('Explicit isolated native browser relay required');

export default defineConfig({
  ...observation,
  testMatch: ['browser-live-chain.check.ts'],
  timeout: 90000,
  outputDir: process.env.CMA_BROWSER_RELAY_DIRECTORY + '/web',
});
