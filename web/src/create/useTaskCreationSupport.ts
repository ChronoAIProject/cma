import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';

export function useTaskCreationSupport(client: CmaWebClient) {
  const [repositorySupported, setRepositorySupported] = useState<boolean | undefined>(() =>
    client.readTaskRepositorySupport ? undefined : false,
  );
  useEffect(() => {
    const abort = new AbortController();
    void client.readTaskRepositorySupport?.({ signal: abort.signal }).then(
      (value) => {
        if (!abort.signal.aborted) setRepositorySupported(value);
      },
      () => {
        if (!abort.signal.aborted) setRepositorySupported(false);
      },
    );
    return () => abort.abort();
  }, [client]);
  const [supported, setSupported] = useState<boolean | undefined>(() =>
    client.readTaskCreationSupport ? undefined : false,
  );
  useEffect(() => {
    const abort = new AbortController();
    if (!client.readTaskCreationSupport) return;
    void client.readTaskCreationSupport({ signal: abort.signal }).then(
      (value) => {
        if (!abort.signal.aborted) setSupported(value);
      },
      () => {
        if (!abort.signal.aborted) setSupported(false);
      },
    );
    return () => abort.abort();
  }, [client]);
  return { supported, repositorySupported };
}
