import { useEffect, useMemo, useState } from 'react';
import {
  connectionClient,
  type ConnectionReport,
  type ConnectionTarget,
} from '../connections/client';

/** Complete optional external sign-in before the owner's final scope approval. */
export function useScopeConnections(
  agentId: string,
  owner: string | undefined,
  services: readonly string[],
  enabled: boolean,
) {
  const signature = JSON.stringify([...services].sort());
  const target = useMemo<ConnectionTarget>(
    () => ({
      type: 'agent_service_access',
      id: agentId,
      services: JSON.parse(signature) as string[],
    }),
    [agentId, signature],
  );
  const scope = useMemo(() => ({ target, owner, enabled }), [target, owner, enabled]);
  const [result, setResult] = useState<{
    scope: object;
    report?: ConnectionReport;
    error?: string;
  }>();
  const [reload, setReload] = useState(0);
  useEffect(() => {
    if (!enabled || !owner) return;
    const controller = new AbortController();
    void connectionClient
      .check(target, undefined, controller.signal)
      .then((report) => {
        if (controller.signal.aborted) return;
        if (report.owner !== owner)
          throw new Error('Your CMA account changed. Open the Agent again.');
        setResult({ scope, report });
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted)
          setResult({
            scope,
            error:
              cause instanceof Error ? cause.message : 'Service connections could not be verified.',
          });
      });
    return () => controller.abort();
  }, [enabled, owner, target, scope, reload]);
  const current = result?.scope === scope ? result : undefined;
  return {
    target,
    ready: enabled && current?.report?.ready === true,
    checking: enabled && !!owner && current === undefined,
    needsConnection: current?.report?.ready === false,
    error: current?.error,
    retry: () => {
      setResult(undefined);
      setReload((value) => value + 1);
    },
    connected: (report: ConnectionReport) => {
      if (report.owner === owner && report.ready) setResult({ scope, report });
    },
  };
}
