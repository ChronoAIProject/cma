import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { request } from '@playwright/test';
import { fixtures, hooks, object, settings } from './environment';
import { credentialValues, prepareEvidenceDirectory } from './security';

export default async function preflight() {
  const config = settings();
  const completeEvidenceSetup = prepareEvidenceDirectory(
    config.output,
    process.env.CMA_E2E_EVIDENCE_RUN ?? '',
  );
  const manifest = config.mode === 'mutation' ? fixtures() : undefined;
  if (manifest !== undefined) {
    credentialValues(manifest.forbiddenValuesFile);
    await hooks(manifest);
    const accounts = [
      ['owner', manifest.accounts.owner],
      ['secondOwner', manifest.accounts.secondOwner],
      ...Object.entries(manifest.visitors),
    ] as const;
    for (const [name, account] of accounts) {
      const api = await request.newContext({
        storageState: account.storageState,
        baseURL: config.baseURL,
      });
      try {
        const response = await api.get('/auth/session');
        const identity = response.ok() ? object(await response.json()) : {};
        if (identity.subject !== account.subject || identity.authenticated !== true)
          throw new Error(`Required E2E authenticated fixture is unavailable: ${name}`);
        const visitor = name !== 'owner' && name !== 'secondOwner';
        if (identity.visitor !== visitor || !Array.isArray(identity.capabilities))
          throw new Error(`Invalid E2E fixture role: ${name}`);
        const capabilities = identity.capabilities as unknown[];
        const required = visitor
          ? ['launch_triggers', 'use_own_sandboxes', 'delete_own_sandbox']
          : name === 'owner'
            ? ['create_sandbox', 'manage_own_profiles', 'publish_triggers', 'delete_own_sandbox']
            : ['manage_own_profiles', 'publish_triggers'];
        if (
          required.some((capability) => !capabilities.includes(capability)) ||
          (visitor &&
            ['create_sandbox', 'manage_own_profiles', 'publish_triggers'].some((capability) =>
              capabilities.includes(capability),
            ))
        )
          throw new Error(`Invalid E2E fixture capabilities or cleanup authority: ${name}`);
      } finally {
        await api.dispose();
      }
    }
  }
  writeFileSync(
    join(config.output, 'provenance.json'),
    JSON.stringify({
      mode: config.mode,
      origin: config.baseURL,
      executedAt: new Date().toISOString(),
      provenance: manifest?.provenance ?? null,
      provenanceSource: manifest === undefined ? 'not_supplied' : 'operator_declared',
      provenanceVerified: false,
      executionTarget: 'configured_remote_origin',
      mocked: false,
    }),
    { mode: 0o600 },
  );
  completeEvidenceSetup();
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, 'evidence_ready=true\n');
}
