import { defineConfig } from '@playwright/test';
import base from './desktop-conversation.config';
import { browserProjects } from './browser-projects';

export default defineConfig({
  ...base,
  projects: browserProjects(),
  testMatch: 'build-identity.check.ts',
});
