import { defineConfig } from '@playwright/test';
import observation from './browser-observation.config';

if (!process.env.CMA_DESKTOP_RELAY_DIRECTORY)
  throw new Error('Explicit isolated desktop creation relay required');

export default defineConfig({
  ...observation,
  testMatch: ['desktop-native.check.ts'],
  timeout: 180000,
  outputDir: process.env.CMA_DESKTOP_RELAY_DIRECTORY + '/web',
  use: {
    ...observation.use,
    launchOptions: {
      executablePath: process.env.CMA_E2E_BROWSER_PATH,
      chromiumSandbox: true,
    },
  },
});
