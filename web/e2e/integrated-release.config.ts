import { defineConfig } from '@playwright/test';
import observation from './browser-observation.config';

if (!process.env.CMA_DESKTOP_RELAY_DIRECTORY) throw new Error('Isolated integrated relay required');

export default defineConfig({
  ...observation,
  testMatch: ['integrated-release.check.ts'],
  timeout: 240000,
  outputDir: process.env.CMA_DESKTOP_RELAY_DIRECTORY + '/web',
  use: {
    ...observation.use,
    actionTimeout: 10000,
    contextOptions: { reducedMotion: 'reduce' },
    launchOptions: {
      executablePath: process.env.CMA_E2E_BROWSER_PATH,
      chromiumSandbox: true,
    },
  },
});
