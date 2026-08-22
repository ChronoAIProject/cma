import { useCallback } from 'react';
import type { Dispatch, SetStateAction } from 'react';
import { readSession } from '../api/client';
import type { Session } from './types';

/**
 * Refresh the fields provisioning settles without discarding a usable view.
 *
 * A create response predates provisioning, which is where the runtime version
 * and installed skills are settled. The fresh record is merged so an older
 * projection cannot erase capabilities the client already read, and a failed
 * refresh changes nothing because the session remains usable either way.
 */
export function useSessionRefresh(
  setSessions: Dispatch<SetStateAction<readonly Session[]>>,
): (id: string) => void {
  return useCallback(
    (id: string) => {
      void readSession(id)
        .then((fresh) => {
          setSessions((previous) =>
            previous.map((session) => (session.id === id ? { ...session, ...fresh } : session)),
          );
        })
        .catch(() => {
          // The client keeps its usable record; the next settle or reload asks again.
        });
    },
    [setSessions],
  );
}
