import { defineConfig } from '@playwright/test';
import { join } from 'node:path';
import setupConfig from './channel-setup.config';

export default defineConfig({
  ...setupConfig,
  testMatch: ['hmac-channel.check.ts'],
  outputDir: join(setupConfig.outputDir!, 'hmac'),
});
