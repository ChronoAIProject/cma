import { defineConfig } from '@playwright/test';
import usage from './usage.config';

export default defineConfig({ ...usage, testMatch: 'spend-cloud.check.ts' });
