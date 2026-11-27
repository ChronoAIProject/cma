import { useEffect, useMemo, useRef, useState } from 'react';
import {
  connectionClient,
  isConnectionReport,
  type ConnectionReport,
  type ConnectionTarget,
} from '../connections/client';
import { observeRead } from './transport/readObserver';
import { readRetryDelay } from './transport/readRetry';

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
  const observer = useRef<ReturnType<typeof observeRead> | undefined>(undefined);
  useEffect(() => {
    if (!enabled || !owner) return;
    const access = observeRead(
      async (signal) => {
        setResult(undefined);
        try {
          const report = await connectionClient.check(target, undefined, signal);
          if (signal.aborted) return;
          if (!isConnectionReport(report))
            throw new Error('Service connections could not be verified.');
          if (report.owner !== owner)
            throw new Error('Your CMA account changed. Open the Agent again.');
          setResult({ scope, report });
          // Connections may be completed outside this tab. Recheck on focus or
          // an explicit retry without polling while the owner is away.
          return report.ready ? undefined : { cooldown: 0 };
        } catch (cause) {
          if (signal.aborted) return;
          setResult({
            scope,
            error:
              cause instanceof Error ? cause.message : 'Service connections could not be verified.',
          });
          return { cooldown: readRetryDelay(cause, 1), failed: true };
        }
      },
      { immediateForeground: true },
    );
    observer.current = access;
    return () => {
      observer.current = undefined;
      access.dispose();
    };
  }, [enabled, owner, target, scope]);
  const current = result?.scope === scope ? result : undefined;
  return {
    target,
    ready: enabled && current?.report?.ready === true,
    checking: enabled && !!owner && current === undefined,
    needsConnection: current?.report?.ready === false,
    missingServices: current?.report?.services.filter((service) => service.state !== 'satisfied'),
    error: current?.error,
    retry: () => observer.current?.refresh(),
    connected: (report: ConnectionReport) => {
      if (isConnectionReport(report) && report.owner === owner && report.ready) {
        observer.current?.dispose();
        setResult({ scope, report });
      }
    },
  };
}
