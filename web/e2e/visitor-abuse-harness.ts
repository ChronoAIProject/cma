import { test as base } from '@playwright/test';
import { settings } from './environment';
import { tc117Fixture } from './visitor-abuse-environment';
import { TC117Client } from './visitor-abuse-client';
import { blocked, check } from './visitor-abuse-model';

export const test = base.extend<{ abuse: TC117Client }>({
  abuse: async ({ playwright }, runTest, info) => {
    void playwright;
    const client = await TC117Client.open(settings(), tc117Fixture());
    let cleanupFailed = false;
    try {
      await runTest(client);
    } finally {
      try {
        await client.close();
      } catch {
        cleanupFailed = true;
      } finally {
        const reason = Object.values(blocked).find((value) =>
          info.errors.some((error) => error.message?.includes(value)),
        );
        if (reason) client.observations.blocked = reason;
        client.writeEvidence(
          info.title.split(' ')[0]!.replace('TC117-', ''),
          cleanupFailed ? 'failed' : reason ? 'blocked' : (info.status ?? 'failed'),
          process.env.CMA_E2E_EVIDENCE_RUN ?? '',
        );
        check(!cleanupFailed, 'scoped cleanup is incomplete; consult cleanup.ndjson');
      }
    }
  },
});
