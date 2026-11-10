import type { FullConfig } from '@playwright/test';
import { parseArgs } from 'node:util';
import preflight from './preflight';
import { requireJourneyMutation } from './j0-j2-policy';

export default async function journeyPreflight(config: FullConfig) {
  // Playwright 1.58 applies CLI repetition during discovery, after global setup.
  // Check the public CLI option too; FullConfig alone still has the project default.
  const { values } = parseArgs({
    options: { 'repeat-each': { type: 'string' } },
    strict: false,
    allowPositionals: true,
  });
  if (
    (values['repeat-each'] !== undefined && values['repeat-each'] !== '1') ||
    config.projects.some((project) => project.retries !== 0 || project.repeatEach !== 1)
  )
    throw new Error('Invalid E2E J0-J2 retry/repetition configuration');
  requireJourneyMutation();
  // Keep the existing account, credential, provenance and cleanup requirements.
  await preflight();
}
