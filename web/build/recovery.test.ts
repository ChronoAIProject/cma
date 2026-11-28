// @vitest-environment node

import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { build } from 'vite';
import { buildIdentityEvidence } from './identity';
import { inputRefusalRecoveryEnabled, recoveryEvidence, replicaRecoveryEnabled } from './recovery';

it('defaults automatic mutation recovery off and accepts only explicit booleans', () => {
  expect(replicaRecoveryEnabled({})).toBe(false);
  expect(inputRefusalRecoveryEnabled({ CHRONO_WEB_REPLICA_RECOVERY_ENABLED: 'true' })).toBe(false);
  expect(inputRefusalRecoveryEnabled({ CHRONO_WEB_INPUT_REFUSAL_RECOVERY_ENABLED: 'true' })).toBe(
    true,
  );
  expect(replicaRecoveryEnabled({ CHRONO_WEB_REPLICA_RECOVERY_ENABLED: 'false' })).toBe(false);
  expect(replicaRecoveryEnabled({ CHRONO_WEB_REPLICA_RECOVERY_ENABLED: 'true' })).toBe(true);
});

it.each(['', 'TRUE', 'False', '1', '0', ' true', 'false '])(
  'rejects invalid activation %j',
  (value) => {
    expect(() => replicaRecoveryEnabled({ CHRONO_WEB_REPLICA_RECOVERY_ENABLED: value })).toThrow(
      'must be true or false',
    );
  },
);

it('keeps the image build and provenance defaults aligned with Vite', () => {
  const dockerfile = readFileSync(new URL('../../web/Dockerfile', import.meta.url), 'utf8');
  expect(dockerfile).toContain('ARG CHRONO_WEB_INPUT_REFUSAL_RECOVERY_ENABLED=false');
  expect(dockerfile).toContain(
    'CHRONO_WEB_INPUT_REFUSAL_RECOVERY_ENABLED=$CHRONO_WEB_INPUT_REFUSAL_RECOVERY_ENABLED',
  );
  expect(dockerfile.match(/^ARG CHRONO_WEB_REPLICA_RECOVERY_ENABLED=false$/gm)).toHaveLength(2);
  expect(dockerfile).toContain(
    'CHRONO_WEB_REPLICA_RECOVERY_ENABLED=$CHRONO_WEB_REPLICA_RECOVERY_ENABLED',
  );
  expect(dockerfile).toContain(
    'io.chronoai.web.replica-recovery-enabled="${CHRONO_WEB_REPLICA_RECOVERY_ENABLED}"',
  );
});

it.each([false, true])('emits matching immutable evidence for recovery %s', async (enabled) => {
  const identity = {
    version: 'git-0123456789ab',
    revision: '0123456789ab'.repeat(3) + '0123',
    local: true,
  };
  const result = await build({
    configFile: false,
    logLevel: 'silent',
    plugins: [
      {
        name: 'recovery-evidence-fixture',
        resolveId: (id) => (id === 'virtual:recovery-fixture' ? id : null),
        load: (id) => (id === 'virtual:recovery-fixture' ? 'export const fixture = true;' : null),
      },
      buildIdentityEvidence(identity),
      recoveryEvidence(identity, enabled, !enabled),
    ],
    build: { write: false, rollupOptions: { input: 'virtual:recovery-fixture' } },
  });
  if ('on' in result || Array.isArray(result)) throw new Error('Expected one completed bundle');
  const evidence = (name: string) => {
    const asset = result.output.find((file) => file.fileName === name);
    if (asset?.type !== 'asset' || typeof asset.source !== 'string')
      throw new Error(`Missing emitted evidence ${name}`);
    return JSON.parse(asset.source);
  };
  expect(evidence('build-info.json')).toEqual(identity);
  expect(evidence('recovery-info.json')).toEqual({
    ...identity,
    replicaRecoveryEnabled: enabled,
    inputRefusalRecoveryEnabled: !enabled,
  });
});
