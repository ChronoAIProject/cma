import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import documents from './documents.config';

export default defineConfig({
  ...documents,
  testMatch: 'documents-edit-links.check.ts',
  outputDir: mkdtempSync(join(tmpdir(), 'cma-documents-edit-links-')),
});
