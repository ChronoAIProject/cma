import { useEffect, useId, useState } from 'react';
import type { DesktopCapability } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';

export function DesktopOption({
  client,
  workspaceId,
  sandboxId,
  enabled,
  frozen,
  onChange,
  onAvailable,
}: {
  readonly client: CmaWebClient;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly enabled: boolean;
  readonly frozen: boolean;
  readonly onChange: (enabled: boolean) => void;
  readonly onAvailable: (available: boolean) => void;
}) {
  const description = useId();
  const [capability, setCapability] = useState<DesktopCapability>();
  const [failure, setFailure] = useState<string | undefined>(() =>
    client.readDesktopCapability ? undefined : 'Desktop creation is unavailable on this service.',
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
          setFailure('Desktop availability could not be checked. Try again later.');
      },
    );
    return () => abort.abort();
  }, [client, workspaceId, sandboxId, onAvailable]);
  return (
    <div>
      <label className="task-home__desktop">
        <input
          type="checkbox"
          checked={enabled}
          disabled={frozen || (!enabled && capability?.state !== 'available')}
          aria-describedby={description}
          onChange={(event) => onChange(event.target.checked)}
        />
        Enable desktop
      </label>
      <p id={description}>
        {frozen
          ? 'Desktop choice is saved with the submitted task.'
          : (failure ??
            (capability
              ? capability.state === 'available'
                ? 'Prepares a browser for this Agent. Requires extra preparation time and memory.'
                : `Desktop unavailable: ${capability.reason}`
              : 'Checking desktop availability…'))}
      </p>
    </div>
  );
}
