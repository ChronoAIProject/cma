import { defineConfig } from '@playwright/test';
import receiptRecovery from './review-receipt-recovery.config';

const preview = new URL(process.env.CMA_RECEIPT_PREVIEW_URL ?? 'http://127.0.0.1:4343');
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
  throw new Error('Launch receipt recovery CI checks require a loopback preview port.');

export default defineConfig({
  ...receiptRecovery,
  outputDir: process.env.CMA_RECEIPT_EVIDENCE_DIR ?? receiptRecovery.outputDir,
  webServer: {
    command: `npm run preview -- --host ${preview.hostname} --port ${preview.port} --strictPort`,
    url: preview.origin,
    reuseExistingServer: false,
    timeout: 30_000,
  },
  use: {
    ...receiptRecovery.use,
    baseURL: preview.origin,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
});
