import type { Plugin } from 'vite';
import type { BuildIdentity } from './identity';

export function replicaRecoveryEnabled(env: Readonly<Record<string, string | undefined>>): boolean {
  const value = env.CHRONO_WEB_REPLICA_RECOVERY_ENABLED ?? 'false';
  if (value !== 'true' && value !== 'false')
    throw new Error('CHRONO_WEB_REPLICA_RECOVERY_ENABLED must be true or false');
  return value === 'true';
}

/** Companion evidence preserves the existing strict build-info.json contract. */
export function recoveryEvidence(identity: BuildIdentity, enabled: boolean): Plugin {
  return {
    name: 'cma-replica-recovery-evidence',
    generateBundle() {
      this.emitFile({
        type: 'asset',
        fileName: 'recovery-info.json',
        source: JSON.stringify({ ...identity, replicaRecoveryEnabled: enabled }),
      });
    },
  };
}
