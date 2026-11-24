import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { downloadAgentHistory } from './historyExport';
import { readRetainedHistory } from './readRetainedHistory';

export function useHistoryExport(client: CmaWebClient, viewerId: string, enabled: boolean) {
  const active = useRef<AbortController | undefined>(undefined);
  const scope = useMemo(() => ({ client, viewerId, enabled }), [client, viewerId, enabled]);
  const [result, setResult] = useState<{
    scope: typeof scope;
    pending: boolean;
    error?: string;
    notice?: string;
  }>({ scope, pending: false });
  useLayoutEffect(() => {
    return () => {
      active.current?.abort();
      active.current = undefined;
    };
  }, [scope]);

  const run = async (agentId: string, retained: boolean) => {
    if (!enabled || active.current) return;
    const operation = new AbortController();
    active.current = operation;
    const timeout = window.setTimeout(() => operation.abort(), 120_000);
    setResult({ scope, pending: true });
    try {
      const source = retained
        ? await readRetainedHistory(client, agentId, operation.signal)
        : await client.exportAgentHistory(agentId, { signal: operation.signal });
      if (operation.signal.aborted) return;
      downloadAgentHistory(agentId, source);
      if (!(source instanceof Blob))
        setResult({
          scope,
          pending: true,
          notice: `Exported recorded messages, tool activity and turn outcomes through sequence ${source.transcript.throughSeq}.`,
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
