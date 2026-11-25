import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { raceWithAbortSignal } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { downloadAgentHistory } from './historyExport';
import { readRetainedHistory, type RetainedHistory } from './readRetainedHistory';

export function useHistoryExport(
  client: CmaWebClient,
  viewerId: string,
  enabled: boolean,
  authority?: object,
) {
  const active = useRef<AbortController | undefined>(undefined);
  const scope = useMemo(
    () => ({ client, viewerId, enabled, authority }),
    [client, viewerId, enabled, authority],
  );
  const current = useRef<typeof scope | undefined>(undefined);
  const [result, setResult] = useState<{
    scope: typeof scope;
    pending: boolean;
    error?: string;
    notice?: string;
  }>({ scope, pending: false });
  useLayoutEffect(() => {
    current.current = scope;
    return () => {
      current.current = undefined;
      active.current?.abort();
      active.current = undefined;
    };
  }, [scope]);

  const run = async (
    agentId: string,
    retained: boolean,
    format: 'history' | 'transcript' = 'history',
  ) => {
    if (!enabled || current.current !== scope || active.current) return;
    const operation = new AbortController();
    active.current = operation;
    const timeout = window.setTimeout(() => operation.abort(), 120_000);
    setResult({ scope, pending: true });
    try {
      const source = await raceWithAbortSignal<Blob | RetainedHistory>(operation.signal, () =>
        retained
          ? readRetainedHistory(client, agentId, operation.signal, format)
          : client.exportAgentHistory(agentId, { signal: operation.signal }),
      );
      if (operation.signal.aborted || current.current !== scope) return;
      downloadAgentHistory(agentId, source);
      if (!(source instanceof Blob))
        setResult({
          scope,
          pending: true,
          notice: source.toolEvents
            ? `Exported recorded messages, tool activity and turn outcomes through sequence ${source.transcript.throughSeq}.`
            : `Exported recorded messages and turn outcomes through sequence ${source.transcript.throughSeq}.`,
        });
    } catch (failure) {
      if (!operation.signal.aborted)
        setResult({
          scope,
          pending: true,
          error: failure instanceof Error ? failure.message : 'History export did not complete.',
        });
    } finally {
      window.clearTimeout(timeout);
      if (active.current === operation) {
        active.current = undefined;
        setResult((previous) => ({
          ...previous,
          pending: false,
          ...(operation.signal.aborted ? { error: 'History export timed out. Please retry.' } : {}),
        }));
      }
    }
  };
  return {
    run,
    pending: result.scope === scope && result.pending,
    error: result.scope === scope ? result.error : undefined,
    notice: result.scope === scope ? result.notice : undefined,
  };
}
