import { defineConfig } from '@playwright/test';
import usage from './usage.config';

export default defineConfig({
  ...usage,
  testMatch: 'spend-controls.check.ts',
  use: { ...usage.use, launchOptions: {} },
  projects: ['chromium', 'firefox', 'webkit'].map((browserName) => ({
    name: browserName,
    use: { browserName: browserName as 'chromium' | 'firefox' | 'webkit' },
  })),
});
