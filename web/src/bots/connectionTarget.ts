import { ApiError } from '@chronoai/cma-sdk';
import type { ConnectionTarget } from '../connections/client';

/**
 * The resource a `connection_required` refusal names, when it names one this
 * client can open the connection wizard for. Anything else — another error,
 * or a target of a shape this build does not know — yields undefined, so the
 * caller reports the refusal instead of guessing at a target.
 */
export function connectionTargetFrom(error: unknown): ConnectionTarget | undefined {
  if (!(error instanceof ApiError) || error.code !== 'connection_required') return undefined;
  const value = error.extensions.connection_target;
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return undefined;
  const target = value as { type?: unknown; id?: unknown; profile?: unknown };
  if (target.type === 'sandbox_profile') {
    if (typeof target.profile !== 'object' || target.profile === null) return undefined;
    const profile = target.profile as { id?: unknown; revision?: unknown };
    return typeof profile.id === 'string' &&
      Number.isSafeInteger(profile.revision) &&
      (profile.revision as number) > 0
      ? {
          type: 'sandbox_profile',
          profile: { id: profile.id, revision: profile.revision as number },
        }
      : undefined;
  }
  if (
    (target.type === 'sandbox' ||
      target.type === 'workspace' ||
      target.type === 'agent' ||
      target.type === 'launch') &&
    typeof target.id === 'string'
  ) {
    return { type: target.type, id: target.id };
  }
  return undefined;
}
