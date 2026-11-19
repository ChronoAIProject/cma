import { useEffect, useState } from 'react';
import type { AgentLifecycleCreateInput, CmaWebClient } from '../api/client';

export function useTaskTitleModeSupport(client: CmaWebClient) {
  const [result, setResult] = useState<{ client: CmaWebClient; supported: boolean }>();
  useEffect(() => {
    const abort = new AbortController();
    void client.readTaskTitleModeSupport?.({ signal: abort.signal }).then(
      (value) => {
        if (!abort.signal.aborted) setResult({ client, supported: value === true });
      },
      () => {
        if (!abort.signal.aborted) setResult({ client, supported: false });
      },
    );
    return () => abort.abort();
  }, [client]);
  return client.readTaskTitleModeSupport
    ? result?.client === client
      ? result.supported
      : undefined
    : false;
}

export function taskCreateRequest(
  draft: {
    readonly text: string;
    readonly machineIncluded: boolean;
    readonly repositoryUrl: string;
    readonly repositoryRef: string;
  },
  context: { readonly workspaceId?: string; readonly sandboxId?: string },
  automaticTitle: boolean,
): AgentLifecycleCreateInput {
  const { workspaceId, sandboxId } = context;
  return {
    name: 'New task',
    ...(automaticTitle ? { title_mode: 'automatic' } : {}),
    initial_message: draft.text,
    ...(draft.machineIncluded ? { machine_included: true } : {}),
    ...(!workspaceId && !sandboxId && draft.repositoryUrl
      ? {
          repository: {
            url: draft.repositoryUrl,
            ...(draft.repositoryRef ? { ref: draft.repositoryRef } : {}),
          },
        }
      : {}),
    ...(workspaceId ? { workspace_id: workspaceId } : {}),
    ...(sandboxId ? { sandbox_id: sandboxId } : {}),
  };
}
