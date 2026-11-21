import { defineConfig } from '@playwright/test';
import feedback from './answer-feedback.config';

export default defineConfig({ ...feedback, testMatch: 'feedback-reasons.check.ts' });
