import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ConnectionReport } from '../connections/client';
import { createPreparedConnectionHandoff, readAgentConnections } from '../connections/preparation';

/** Readiness can overlap loading, but belongs only to this authorized view generation. */
export function useAgentConnectionReadiness(
  agentId: string,
  owner: string | undefined,
  context: object | undefined,
  enabled: boolean,
) {
  const [revision, setRevision] = useState(0);
  const identityScope = useMemo(() => ({ agentId, owner, context }), [agentId, owner, context]);
  const scope = useMemo(
    () => ({ ...identityScope, identityScope, enabled, revision }),
    [identityScope, enabled, revision],
  );
  const contradicted = useRef<typeof identityScope | undefined>(undefined);
  const [ownerMismatch, setOwnerMismatch] = useState<typeof identityScope>();
  const active = useRef<typeof scope | undefined>(undefined);
  const ownerIdentityCurrent = useCallback(
    () =>
      owner !== undefined &&
      active.current?.identityScope === identityScope &&
      active.current.enabled &&
      contradicted.current !== identityScope,
    [identityScope, owner],
  );
  const prepared = useRef<
    | {
        readonly scope: typeof scope;
        readonly report: ConnectionReport;
        readonly take: () => ConnectionReport | undefined;
      }
    | undefined
  >(undefined);
  const visibilityEpoch = useRef(0);
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
      prepared.current = undefined;
    };
  }, [scope]);
  const discardPrepared = useCallback(() => {
    visibilityEpoch.current += 1;
    prepared.current = undefined;
  }, []);
  useEffect(() => {
    document.addEventListener('visibilitychange', discardPrepared);
    window.addEventListener('blur', discardPrepared);
    return () => {
      document.removeEventListener('visibilitychange', discardPrepared);
      window.removeEventListener('blur', discardPrepared);
    };
  }, [discardPrepared]);
  const accept = useCallback(
    (report: ConnectionReport) => {
      if (!enabled || active.current !== scope) return;
      prepared.current = undefined;
      if (owner !== undefined && report.owner !== owner) {
        contradicted.current = identityScope;
        setOwnerMismatch(identityScope);
        setResult({
          scope,
          error: 'Your CMA account changed. Open the Agent again to continue.',
        });
        return;
      }
      if (owner !== undefined && report.owner === owner) {
        contradicted.current = undefined;
        setOwnerMismatch(undefined);
      }
      if (report.ready) setResult({ scope, verified: report });
      else if (report.services.some((row) => row.state === 'missing' || row.state === 'inactive'))
        setResult({ scope, needed: true });
      else setResult({ scope, error: 'The connections could not be verified. Please try again.' });
    },
    [enabled, identityScope, owner, scope],
  );
  useEffect(() => {
    if (!enabled) return;
    const abort = new AbortController();
    const epoch = visibilityEpoch.current;
    void readAgentConnections(context, agentId, owner, abort.signal)
      .then((observation) => {
        if (abort.signal.aborted || active.current !== scope) return;
        const { report } = observation;
        accept(report);
        if (
          observation.prepared &&
          report.ready &&
          owner !== undefined &&
          report.owner === owner &&
          epoch === visibilityEpoch.current &&
          !document.hidden
        )
          prepared.current = { scope, report, take: createPreparedConnectionHandoff(observation) };
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
  }, [accept, agentId, context, enabled, owner, scope]);
  const current = result?.scope === scope && enabled ? result : undefined;
  return {
    scope,
    verified: current?.verified,
    ownerMismatch: ownerMismatch === identityScope,
    ownerIdentityCurrent,
    needed: current?.needed === true,
    error: current?.error,
    accept,
    discardPrepared,
    takePrepared: () => {
      const value = prepared.current;
      prepared.current = undefined;
      if (
        !enabled ||
        document.hidden ||
        active.current !== scope ||
        value?.scope !== scope ||
        value.report !== current?.verified
      )
        return undefined;
      return value.take();
    },
    clearError: () => {
      if (active.current === scope)
        setResult((value) => (value?.scope === scope ? { ...value, error: undefined } : value));
    },
    retry: () => setRevision((value) => value + 1),
  };
}
