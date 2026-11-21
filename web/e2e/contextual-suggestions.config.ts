import { defineConfig } from '@playwright/test';
import suggestions from './suggestions.config';

export default defineConfig({ ...suggestions, testMatch: 'contextual-suggestions.check.ts' });
