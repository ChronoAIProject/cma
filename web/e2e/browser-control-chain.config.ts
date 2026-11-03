import { defineConfig } from '@playwright/test';
import observation from './browser-observation.config';

if (!process.env.CMA_BROWSER_RELAY_DIRECTORY || !process.env.CMA_CONTROL_PROOF_MANIFEST)
  throw new Error('Explicit isolated control relay and candidate manifest required');

export default defineConfig({
  ...observation,
  testMatch: ['browser-control-chain.check.ts'],
  timeout: 180000,
  outputDir: process.env.CMA_BROWSER_RELAY_DIRECTORY + '/web',
});
