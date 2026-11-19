import { defineConfig } from '@playwright/test';
import provenance from './provenance.config';

export default defineConfig({
  ...provenance,
  testMatch: 'channel-outbound.check.ts',
  timeout: 60_000,
});
