import { defineConfig } from '@playwright/test';
import documents from './documents.config';

export default defineConfig({ ...documents, testMatch: 'agent-failure.check.ts' });
