import { useEffect, useMemo, useState } from 'react';
import type { CmaWebClient } from '../api/client';

export function usePromptAvailability(
  client: CmaWebClient,
  agentId: string,
  viewerId: string | undefined,
  enabled: boolean,
) {
  const scope = useMemo(
    () => ({ client, agentId, viewerId, enabled }),
    [client, agentId, viewerId, enabled],
  );
  const [result, setResult] = useState<{ scope: object; reason?: string }>();
  useEffect(() => {
    if (!enabled || !client.readAgentPreferences) return;
    const abort = new AbortController();
    const read = async () => {
      try {
        const preferences = await client.readAgentPreferences!(agentId, { signal: abort.signal });
        if (!abort.signal.aborted)
          setResult({
            scope,
            reason: preferences.prompt_supported
              ? undefined
              : 'This runtime does not support system prompt updates. Include instructions in your next message instead.',
          });
      } catch {
        if (!abort.signal.aborted)
          setResult({
            scope,
            reason:
              'Could not check system prompt support. Close and reopen this menu to check again.',
          });
      }
    };
    void read();
    return () => abort.abort();
  }, [client, agentId, enabled, scope]);
  return result?.scope === scope ? result.reason : 'Checking system prompt support…';
}
