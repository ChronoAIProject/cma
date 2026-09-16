import { defineConfig } from '@playwright/test';
import provenance from './provenance.config';
export default defineConfig({ ...provenance, testMatch: 'artifacts.check.ts', timeout: 60_000 });
