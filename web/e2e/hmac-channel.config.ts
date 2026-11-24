import { defineConfig } from '@playwright/test';
import setupConfig from './channel-setup.config';

export default defineConfig({
  ...setupConfig,
  testMatch: ['hmac-channel.check.ts'],
});
