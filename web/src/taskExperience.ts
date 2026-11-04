import { useEffect, useMemo, useState } from 'react';
import type { TaskExperienceCapabilities } from '@chronoai/cma-sdk';
import type { CmaWebClient } from './api/client';

const disabled: TaskExperienceCapabilities = {
  task_home_admission: false,
  suggested_followup_admission: false,
  desktop_enable: false,
  browser_takeover: false,
  replay_presentation: false,
};

export function useTaskExperience(client: CmaWebClient, accountId?: string) {
  const scope = useMemo(() => ({ client, accountId }), [client, accountId]);
  const [result, setResult] = useState<{
    scope: object;
    capabilities: TaskExperienceCapabilities;
  }>();
  useEffect(() => {
    let request: AbortController | undefined;
    const refresh = () => {
      request?.abort();
      const current = new AbortController();
      request = current;
      setResult(undefined);
      if (!accountId || !client.taskExperience) return;
      void client.taskExperience.read({ signal: current.signal }).then(
        (value) => {
          if (!current.signal.aborted)
            setResult({
              scope,
              capabilities: value.account_id === accountId ? value.capabilities : disabled,
            });
        },
        () => {
          if (!current.signal.aborted) setResult({ scope, capabilities: disabled });
        },
      );
    };
    const visible = () => {
      if (document.visibilityState !== 'hidden') refresh();
    };
    const offline = () => {
      request?.abort();
      setResult(undefined);
    };
    refresh();
    window.addEventListener('focus', refresh);
    window.addEventListener('online', refresh);
    window.addEventListener('offline', offline);
    document.addEventListener('visibilitychange', visible);
    return () => {
      request?.abort();
      window.removeEventListener('focus', refresh);
      window.removeEventListener('online', refresh);
      window.removeEventListener('offline', offline);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [scope, client, accountId]);
  return result?.scope === scope ? result.capabilities : disabled;
}
