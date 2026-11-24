import { defineConfig } from '@playwright/test';
import { join } from 'node:path';
import setupConfig from './channel-setup.config';
export default defineConfig({
  ...setupConfig,
  testMatch: ['wh-eg-authoring.check.ts'],
  outputDir: join(setupConfig.outputDir!, 'authoring'),
  timeout: 60_000,
});
