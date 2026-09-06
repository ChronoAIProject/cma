import { useEffect, useState } from 'react';
import type { CapacityBounds, CapacityBoundsClient } from './client';

interface BoundsState {
  readonly client?: CapacityBoundsClient;
  readonly bounds?: CapacityBounds;
  readonly loading: boolean;
  readonly failed: boolean;
}

export function useCapacityBounds(client?: CapacityBoundsClient) {
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<BoundsState>({
    client,
    loading: client !== undefined,
    failed: false,
  });
  useEffect(() => {
    if (client === undefined) return;
    const request = new AbortController();
    void client.readCapacityBounds({ signal: request.signal }).then(
      (bounds) => {
        if (!request.signal.aborted) setState({ client, bounds, loading: false, failed: false });
      },
      () => {
        if (!request.signal.aborted) setState({ client, loading: false, failed: true });
      },
    );
    return () => request.abort();
  }, [client, reload]);
  return {
    ...(state.client === client
      ? state
      : { loading: client !== undefined, failed: false, bounds: undefined }),
    retry: () => {
      setState({ client, loading: true, failed: false });
      setReload((value) => value + 1);
    },
  };
}
