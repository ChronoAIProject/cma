import { defineConfig } from '@playwright/test';
import credentialRecovery from './credential-setup.config';

export default defineConfig({
  ...credentialRecovery,
  testMatch: [
    'review-receipt-recovery.check.ts',
    'review-recovery-custody.check.ts',
    'review-acceptance-order.check.ts',
    'review-progress-order.check.ts',
    'review-preflight-lifetime.check.ts',
  ],
});
