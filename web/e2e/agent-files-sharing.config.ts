import { defineConfig } from '@playwright/test';
import provenance from './provenance.config';

export default defineConfig({
  ...provenance,
  testMatch: ['agent-files-sharing.check.ts', 'agent-share-sign-in.check.ts'],
  timeout: 60_000,
});
