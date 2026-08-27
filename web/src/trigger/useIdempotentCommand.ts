import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  BrowserCommandLedger,
  COMMAND_LEDGER_EVENT,
  COMMAND_STORAGE_PREFIX,
  commandOutcomeIsUnresolved,
} from './BrowserCommandLedger';
import type { BrowserCommandIntent, PendingBrowserCommand } from './BrowserCommandLedger';

export {
  BrowserCommandLedger,
  COMMAND_LEASE_MS,
  COMMAND_RETENTION_MS,
  CommandDescriptorMismatchError,
  CommandLedgerUnavailableError,
  CommandRequestUnavailableError,
  commandOutcomeIsUnresolved,
  retainedCommandMetadata,
  retainedCommandRequest,
} from './BrowserCommandLedger';
export type { BrowserCommandIntent, PendingBrowserCommand } from './BrowserCommandLedger';

interface IdempotentCommands {
  readonly pending: PendingBrowserCommand | null;
  readonly execute: <TRequest, TResult>(
    intent: BrowserCommandIntent<TRequest>,
    label: string,
    send: (key: string, request: TRequest) => Promise<TResult>,
  ) => Promise<TResult>;
}

/**
 * One immediate same-key replay, then an explicit durable reconciliation
 * state shared by every same-origin tab for this owner and resource.
 */
export function useIdempotentCommand(
  ownerId: string,
  resources: readonly string[],
): IdempotentCommands {
  const ledger = useMemo(() => new BrowserCommandLedger({ ownerId }), [ownerId]);
  const [pending, setPending] = useState<PendingBrowserCommand | null>(null);
  const resourcesKey = resources.join('\u0000');
  const currentResources = useRef<readonly string[]>([]);

  useEffect(() => {
    currentResources.current = resourcesKey === '' ? [] : resourcesKey.split('\u0000');
  }, [resourcesKey]);

  const refreshPending = useCallback(async (): Promise<void> => {
    setPending(await ledger.pending(currentResources.current));
  }, [ledger]);

  useEffect(() => {
    let live = true;
    const refresh = () => {
      void ledger.pending(currentResources.current).then((next) => {
        if (live) setPending(next);
      });
    };
    const storageChanged = (event: StorageEvent) => {
      if (event.key === null || event.key.startsWith(COMMAND_STORAGE_PREFIX)) refresh();
    };
    const ledgerChanged = () => refresh();
    window.addEventListener('storage', storageChanged);
    window.addEventListener(COMMAND_LEDGER_EVENT, ledgerChanged);
    refresh();
    return () => {
      live = false;
      window.removeEventListener('storage', storageChanged);
      window.removeEventListener(COMMAND_LEDGER_EVENT, ledgerChanged);
    };
  }, [ledger, resourcesKey]);

  const execute = useCallback(
    async <TRequest, TResult>(
      intent: BrowserCommandIntent<TRequest>,
      label: string,
      send: (key: string, request: TRequest) => Promise<TResult>,
    ): Promise<TResult> => {
      const attempt = () => ledger.execute(intent, label, send);
      try {
        const result = await attempt();
        await refreshPending();
        return result;
      } catch (cause) {
        await refreshPending();
        if (!commandOutcomeIsUnresolved(cause)) throw cause;
      }

      try {
        const result = await attempt();
        await refreshPending();
        return result;
      } catch (cause) {
        await refreshPending();
        throw cause;
      }
    },
    [ledger, refreshPending],
  );

  return { pending, execute };
}
