import { defineConfig } from '@playwright/test';
import credentialRecovery from './credential-setup.config';

const preview = new URL(process.env.CMA_AUTH_REFRESH_PREVIEW_URL ?? 'http://127.0.0.1:4361');
if (
  preview.protocol !== 'http:' ||
  !['127.0.0.1', 'localhost'].includes(preview.hostname) ||
  !preview.port ||
  preview.username ||
  preview.password ||
  preview.pathname !== '/' ||
  preview.search ||
  preview.hash
)
  throw new Error('Identity refresh checks require a loopback preview port.');

export default defineConfig({
  ...credentialRecovery,
  testMatch: [
    'auth-refresh-followup.check.ts',
    'auth-sign-out.check.ts',
    'auth-sign-out-transport.check.ts',
  ],
  outputDir: process.env.CMA_AUTH_REFRESH_EVIDENCE_DIR ?? credentialRecovery.outputDir,
  webServer: {
    command: `npm run preview -- --host ${preview.hostname} --port ${preview.port} --strictPort`,
    url: preview.origin,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  use: {
    ...credentialRecovery.use,
    baseURL: preview.origin,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
});
