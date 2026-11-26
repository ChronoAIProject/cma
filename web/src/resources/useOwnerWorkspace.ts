import { rateLimitDelay } from '../agent/transport/readRetry';
import { useEffect, useState } from 'react';
import { ApiError } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { readableError } from '../vocabulary/errors';
import type { SandboxDetail, WorkspaceDetail } from '../api/types';
import { sandboxRefreshDelay } from '../sandbox/lifecycle';
import { createPagePollDelay, sandboxPollKey } from '../sandbox/pollDelay';

export type OwnerWorkspaceState =
  | { readonly status: 'loading' }
  | { readonly status: 'error'; readonly message: string }
  | {
      readonly status: 'ready';
      readonly workspace: WorkspaceDetail;
      readonly sandbox: SandboxDetail;
      /** The last read failed; what is shown is the last confirmed observation. */
      readonly pollError: boolean;
    };

/**
 * The owner's Workspace and its parent Sandbox, kept current.
 *
 * Both come from the public owner reads the Sandbox page uses, on the same
 * cadence: readiness backs off while an observation stays unchanged, up to
 * thirty seconds after ten minutes; settled reads stay at five seconds. Reads
 * never overlap and are deferred while the page is hidden. A transient failure keeps the last
 * observation and marks it stale; only a refusal or a missing resource
 * replaces it, because a failed read is never proof that something is gone.
 */
export function useOwnerWorkspace(client: CmaWebClient, workspaceId: string, revision: number) {
  const [retry, setRetry] = useState(0);
  const [stored, setStored] = useState<{
    readonly key: string;
    readonly state: OwnerWorkspaceState;
  }>();
  const key = `${workspaceId}:${String(revision)}:${String(retry)}`;
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    let retryAt = 0;
    let failures = 0;
    const delay = createPagePollDelay();
    const poll = () => {
      if (abort.signal.aborted || inFlight || document.hidden) return;
      clearTimeout(timer);
      if (Date.now() < retryAt) {
        timer = setTimeout(poll, retryAt - Date.now());
        return;
      }
      inFlight = true;
      void (async () => {
        try {
          const workspace = await client.readWorkspace(workspaceId, { signal: abort.signal });
          if (workspace.id !== workspaceId)
            throw new Error('Workspace projection referred to another resource.');
          const sandbox = await client.readSandbox(workspace.sandboxId, { signal: abort.signal });
          if (sandbox.id !== workspace.sandboxId)
            throw new Error('Sandbox projection referred to another resource.');
          if (abort.signal.aborted) return;
          failures = 0;
          retryAt = 0;
          setStored({ key, state: { status: 'ready', workspace, sandbox, pollError: false } });
          timer = setTimeout(
            poll,
            delay.next(
              JSON.stringify([sandboxPollKey(sandbox), workspace.status]),
              sandboxRefreshDelay(sandbox, ['preparing', 'removing'].includes(workspace.status)),
            ),
          );
        } catch (error) {
          if (abort.signal.aborted) return;
          const denied = error instanceof ApiError && [401, 403, 404, 410].includes(error.status);
          setStored((previous) => ({
            key,
            state:
              !denied && previous?.state.status === 'ready'
                ? { ...previous.state, pollError: true }
                : {
                    status: 'error',
                    message: readableError(error, 'Workspace unavailable.'),
                  },
          }));
          retryAt = Date.now() + rateLimitDelay(error, 5000, ++failures);
          timer = setTimeout(poll, retryAt - Date.now());
        } finally {
          inFlight = false;
        }
      })();
    };
    const visible = () => {
      clearTimeout(timer);
      if (document.hidden) return;
      delay.reset();
      poll();
    };
    document.addEventListener('visibilitychange', visible);
    poll();
    return () => {
      abort.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [client, workspaceId, key]);
  // A newer request keeps showing the last observation until it answers, but
  // that observation no longer grants actions: it is revalidating.
  const state: OwnerWorkspaceState =
    stored?.key === key || stored?.state.status === 'ready' ? stored.state : { status: 'loading' };
  return {
    state,
    revalidating: stored?.key !== key,
    retry: () => setRetry((value) => value + 1),
  };
}
