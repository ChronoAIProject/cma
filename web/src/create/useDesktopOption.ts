import { useEffect, useState } from 'react';
import type { DesktopCapability } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';

export function useDesktopOption({
  client,
  workspaceId,
  sandboxId,
  enabled,
  frozen,
  onAvailable,
  policyAllowed = false,
}: {
  readonly client: CmaWebClient;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly enabled: boolean;
  readonly frozen: boolean;
  readonly onAvailable: (available: boolean) => void;
  readonly policyAllowed?: boolean;
}) {
  const [capability, setCapability] = useState<DesktopCapability>();
  const [failure, setFailure] = useState<string | undefined>(() =>
    client.readDesktopCapability
      ? undefined
      : 'Agent browser creation is unavailable on this service.',
  );
  useEffect(() => {
    const abort = new AbortController();
    onAvailable(false);
    const request = client.readDesktopCapability?.(
      { workspace_id: workspaceId, sandbox_id: sandboxId },
      { signal: abort.signal },
    );
    void request?.then(
      (value) => {
        if (abort.signal.aborted) return;
        setCapability(value);
        onAvailable(value.state === 'available');
      },
      () => {
        if (!abort.signal.aborted)
          setFailure('Agent browser availability could not be checked. Try again later.');
      },
    );
    return () => abort.abort();
  }, [client, workspaceId, sandboxId, onAvailable]);
  return {
    id: 'browser',
    role: 'menuitemcheckbox' as const,
    label: 'Agent browser',
    disabled: frozen || (!enabled && (!policyAllowed || capability?.state !== 'available')),
    reason: frozen
      ? 'Agent browser choice is saved with the submitted task.'
      : !policyAllowed
        ? 'Agent browser is unavailable. Your current choice is kept.'
        : (failure ??
          (capability
            ? capability.state === 'available'
              ? 'Prepares a browser for this Agent. Requires extra preparation time and memory.'
              : `Agent browser unavailable: ${capability.reason}`
            : 'Checking Agent browser availability…')),
  };
}
