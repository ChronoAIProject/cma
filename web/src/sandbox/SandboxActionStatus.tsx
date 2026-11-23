import { useEffect, useRef, useState } from 'react';
import { ApiError } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import type { AppRoute } from '../routing/routes';
import type { TreeAction } from '../shell/Tree';
import { Button } from '../ui/Button';
import { transitionRefreshDelay } from './lifecycle';
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';

export interface SandboxActionRequest {
  readonly action: Extract<TreeAction, { kind: 'pause_sandbox' | 'revive_sandbox' }>;
  readonly key: string;
  readonly phase: 'submitting' | 'accepted' | 'uncertain' | 'complete' | 'failed';
  readonly hidden?: boolean;
}

export function SandboxActionStatus({
  client,
  request,
  onSettled,
  onClose,
  onNavigate,
}: {
  readonly client: CmaWebClient;
  readonly request: SandboxActionRequest;
  readonly onSettled: (complete: boolean) => void;
  readonly onClose: () => void;
  readonly onNavigate?: (route: AppRoute) => void;
}) {
  const [error, setError] = useState<string>();
  const settled = useRef(onSettled);
  useEffect(() => {
    settled.current = onSettled;
  }, [onSettled]);
  const resume = request.action.kind === 'revive_sandbox';
  const sandboxId = request.action.sandboxId;
  useEffect(() => {
    if (request.phase !== 'accepted') return;
    const controller = new AbortController();
    const startedAt = Date.now();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let reading = false;
    let retryAt = 0;
    let finished = false;
    const poll = () => {
      if (controller.signal.aborted || reading || finished || document.hidden) return;
      clearTimeout(timer);
      if (Date.now() < retryAt) {
        timer = setTimeout(poll, retryAt - Date.now());
        return;
      }
      reading = true;
      void client
        .readSandbox(sandboxId, { signal: controller.signal })
        .then((sandbox) => {
          if (controller.signal.aborted) return;
          if (sandbox.id !== sandboxId)
            throw new Error('Sandbox progress referred to another resource.');
          setError(undefined);
          retryAt = 0;
          if (
            sandbox.status === (resume ? 'active' : 'paused') &&
            sandbox.lifecycle?.operation === undefined
          ) {
            finished = true;
            settled.current(true);
          } else if (sandboxHasParkedRuntimeUpdate(sandbox)) {
            finished = true;
            setError('A runtime update needs attention. Open Sandbox details to retry it.');
            settled.current(false);
          } else if (['failed', 'retiring', 'retired'].includes(sandbox.status)) {
            finished = true;
            setError(
              sandbox.failure?.remediation ??
                'The Sandbox could not finish this request. Open its progress for details.',
            );
            settled.current(false);
          } else timer = setTimeout(poll, transitionRefreshDelay(startedAt));
        })
        .catch((cause: unknown) => {
          if (controller.signal.aborted) return;
          if (cause instanceof ApiError && [401, 403, 404].includes(cause.status)) {
            finished = true;
            setError('Sandbox progress is no longer accessible. Completion is not confirmed.');
            settled.current(false);
          } else {
            setError(
              'Sandbox progress could not be refreshed. Reconnecting; completion is not confirmed.',
            );
            retryAt =
              Date.now() +
              Math.max(5000, cause instanceof ApiError ? (cause.retryAfterMs ?? 0) : 0);
            timer = setTimeout(poll, retryAt - Date.now());
          }
        })
        .finally(() => {
          reading = false;
        });
    };
    document.addEventListener('visibilitychange', poll);
    poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', poll);
    };
  }, [client, request.key, request.phase, resume, sandboxId]);
  // Hiding the notice only changes presentation; confirmed state is still
  // observed so the target's controls can unlock without a page reload.
  if (request.hidden && request.phase !== 'failed' && error === undefined) return null;
  const action = resume ? 'Resume' : 'Pause';
  const progressLink =
    onNavigate === undefined ? null : (
      <Button onClick={() => onNavigate({ kind: 'sandbox', sandboxId })}>
        View Sandbox progress
      </Button>
    );
  return (
    <section className="console__navigation-status" aria-label="Sandbox request">
      <p role="status">
        {request.phase === 'submitting'
          ? `Submitting ${action.toLowerCase()} request…`
          : request.phase === 'accepted'
            ? `${action} request accepted.`
            : request.phase === 'complete'
              ? `Sandbox ${resume ? 'resumed' : 'paused'}.`
              : request.phase === 'uncertain'
                ? `${action} request is not confirmed. Retry the same action to check its result.`
                : 'Sandbox request needs attention.'}
      </p>
      {error === undefined ? null : <p role="alert">{error}</p>}
      {request.phase === 'accepted' ? (
        <details>
          <summary>Operation progress</summary>
          <p>Waiting for the Sandbox to {resume ? 'resume' : 'pause'}.</p>
          {progressLink}
        </details>
      ) : (
        progressLink
      )}
      {request.phase !== 'submitting' ? <Button onClick={onClose}>Dismiss status</Button> : null}
    </section>
  );
}
