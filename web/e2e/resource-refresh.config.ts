import { defineConfig } from '@playwright/test';
import opening from './agent-open.config';

export default defineConfig({
  ...opening,
  testMatch: 'resource-refresh.check.ts',
  use: { ...opening.use, trace: 'off' },
  outputDir: process.env.CMA_REFRESH_OUTPUT ?? '../test-results/resource-refresh',
});
