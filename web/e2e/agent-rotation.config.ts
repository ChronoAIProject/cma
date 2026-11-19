import { defineConfig } from '@playwright/test';
import base from './desktop-conversation.config';

if (!base.webServer || Array.isArray(base.webServer))
  throw new Error('Rotation checks require one local preview server.');

export default defineConfig({
  ...base,
  testMatch: 'agent-rotation.check.ts',
  webServer: { ...base.webServer, reuseExistingServer: false },
});
