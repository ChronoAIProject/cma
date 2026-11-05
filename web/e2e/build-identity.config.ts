import { defineConfig } from '@playwright/test';
import base from './desktop-conversation.config';

export default defineConfig({
  ...base,
  testMatch: 'build-identity.check.ts',
});
