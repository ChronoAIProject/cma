import { defineConfig } from '@playwright/test';
import credentialRecovery from './credential-setup.config';

export default defineConfig({
  ...credentialRecovery,
  testMatch: ['review-receipt-recovery.check.ts', 'review-preflight-lifetime.check.ts'],
});
