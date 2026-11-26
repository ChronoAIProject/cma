import { defineConfig } from '@playwright/test';
import provenance from './provenance.config';
export default defineConfig({
  ...provenance,
  testMatch: 'revocation-recovery.check.ts',
  timeout: 60_000,
});
