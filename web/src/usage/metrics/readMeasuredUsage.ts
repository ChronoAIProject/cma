import type { Cma } from '@chronoai/cma-sdk';
import type { ReadMeasuredUsage } from './useMeasuredUsage';

/** Session exposure works for owners and admins without granting settings access. */
export function measuredUsageReader(auth: Cma['auth']): ReadMeasuredUsage {
  return async (options) => {
    const session = await auth.readSession(options);
    return session.authenticated && session.usage_reporting?.measured_visible === true;
  };
}
