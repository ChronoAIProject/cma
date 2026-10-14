import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { connectionClient, type ConnectionReport } from '../connections/client';

/** Readiness can overlap loading, but belongs only to this authorized view generation. */
export function useAgentConnectionReadiness(
  agentId: string,
  owner: string | undefined,
  context: object | undefined,
  enabled: boolean,
) {
  const [revision, setRevision] = useState(0);
  const scope = useMemo(
    () => ({ agentId, owner, context, enabled, revision }),
    [agentId, owner, context, enabled, revision],
  );
  const active = useRef<typeof scope | undefined>(undefined);
  const [result, setResult] = useState<{
    readonly scope: typeof scope;
    readonly verified?: ConnectionReport;
    readonly needed?: boolean;
    readonly error?: string;
  }>();
  useLayoutEffect(() => {
    active.current = scope;
    return () => {
      active.current = undefined;
    };
  }, [scope]);
  const accept = useCallback(
    (report: ConnectionReport) => {
      if (!enabled || active.current !== scope) return;
      if (owner !== undefined && report.owner !== owner)
        setResult({
          scope,
          error: 'Your CMA account changed. Open the Agent again to continue.',
        });
      else if (report.ready) setResult({ scope, verified: report });
      else if (report.services.some((row) => row.state === 'missing' || row.state === 'inactive'))
        setResult({ scope, needed: true });
      else setResult({ scope, error: 'The connections could not be verified. Please try again.' });
    },
    [enabled, owner, scope],
  );
  useEffect(() => {
    if (!enabled) return;
    const abort = new AbortController();
    void connectionClient
      .check({ type: 'agent', id: agentId }, undefined, abort.signal)
      .then((report) => {
        if (!abort.signal.aborted) accept(report);
      })
      .catch((error: unknown) => {
        if (!abort.signal.aborted && active.current === scope)
          setResult({
            scope,
            error:
              error instanceof Error ? error.message : 'The connections could not be verified.',
          });
      });
    return () => abort.abort();
  }, [accept, agentId, enabled, scope]);
  const current = result?.scope === scope && enabled ? result : undefined;
  return {
    scope,
    verified: current?.verified,
    needed: current?.needed === true,
    error: current?.error,
    accept,
    clearError: () => {
      if (active.current === scope)
        setResult((value) => (value?.scope === scope ? { ...value, error: undefined } : value));
    },
    retry: () => setRevision((value) => value + 1),
  };
}
