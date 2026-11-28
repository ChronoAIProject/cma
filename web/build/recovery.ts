import type { Plugin } from 'vite';
import type { BuildIdentity } from './identity';

export function replicaRecoveryEnabled(env: Readonly<Record<string, string | undefined>>): boolean {
  return recoveryFlag(env, 'CHRONO_WEB_REPLICA_RECOVERY_ENABLED');
}

export function inputRefusalRecoveryEnabled(env: Readonly<Record<string, string | undefined>>) {
  return recoveryFlag(env, 'CHRONO_WEB_INPUT_REFUSAL_RECOVERY_ENABLED');
}

function recoveryFlag(env: Readonly<Record<string, string | undefined>>, name: string): boolean {
  const value = env[name] ?? 'false';
  if (value !== 'true' && value !== 'false') throw new Error(`${name} must be true or false`);
  return value === 'true';
}

/** Companion evidence preserves the existing strict build-info.json contract. */
export function recoveryEvidence(
  identity: BuildIdentity,
  enabled: boolean,
  inputRefusalEnabled: boolean,
): Plugin {
  return {
    name: 'cma-replica-recovery-evidence',
    generateBundle() {
      this.emitFile({
        type: 'asset',
        fileName: 'recovery-info.json',
        source: JSON.stringify({
          ...identity,
          replicaRecoveryEnabled: enabled,
          inputRefusalRecoveryEnabled: inputRefusalEnabled,
        }),
      });
    },
  };
}
