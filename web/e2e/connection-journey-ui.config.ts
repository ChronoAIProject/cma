import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import startup from './agent-startup.config';

const output = mkdtempSync(join(tmpdir(), 'cma-connection-wizard-build-'));
process.once('exit', () => rmSync(output, { recursive: true, force: true }));
const quotedOutput = "'" + output.replaceAll("'", "'\\''") + "'";
const baseURL = 'http://127.0.0.1:4357';
export default defineConfig(startup, {
  testMatch: [
    'connection-journey-ui.check.ts',
    'connection-journey-ui-modes.check.ts',
    'connection-popup-blocked.check.ts',
    'connection-cancel-reentry.check.ts',
    'connection-terminal-reentry.check.ts',
    'connection-pending-reentry.check.ts',
  ],
  metadata: { connectionWizardBuild: output },
  use: { baseURL },
  webServer: {
    command: `npm run build -- --outDir ${quotedOutput} && npm run preview -- --outDir ${quotedOutput} --host 127.0.0.1 --port 4357 --strictPort`,
    url: baseURL,
    timeout: 120_000,
    reuseExistingServer: false,
    env: { VITE_CONNECTION_INTENT_RECOVERY_ENABLED: 'true' },
  },
});
