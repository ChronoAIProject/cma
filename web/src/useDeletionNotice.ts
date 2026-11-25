import { useState } from 'react';
import type { DeletionRequest } from './deletion';

export interface DeletionNoticeContext {
  readonly view: string;
  readonly environment: boolean;
}

const fallback = new Map<string, readonly string[]>();
const storageKey = (viewerId: string) => `cma:deletion-notice:v1:${encodeURIComponent(viewerId)}`;
const identity = (request: DeletionRequest) => {
  const action = request.action;
  return JSON.stringify([
    request.key,
    action.kind,
    action.kind === 'delete_sandbox'
      ? action.sandboxId
      : action.kind === 'delete_agent'
        ? action.agentId
        : action.workspaceId,
  ]);
};

function readDismissed(viewerId: string): readonly string[] {
  const memory = fallback.get(viewerId);
  if (memory) return memory;
  try {
    const value: unknown = JSON.parse(sessionStorage.getItem(storageKey(viewerId)) ?? '[]');
    return Array.isArray(value) && value.every((item) => typeof item === 'string') ? value : [];
  } catch {
    return [];
  }
}

export function useDeletionNotice(
  viewerId: string,
  requests: readonly DeletionRequest[],
  context: DeletionNoticeContext,
) {
  const [scope, setScope] = useState(() => ({
    viewerId,
    view: context.view,
    origin: {},
    announced: undefined as string | undefined,
    dismissed: readDismissed(viewerId),
    open: false,
  }));
  if (scope.viewerId !== viewerId || scope.view !== context.view) {
    setScope({
      viewerId,
      view: context.view,
      origin: {},
      announced: undefined,
      dismissed: scope.viewerId === viewerId ? scope.dismissed : readDismissed(viewerId),
      open: false,
    });
  } else if (scope.open && requests.length === 0) {
    setScope({ ...scope, open: false });
  }
  const eligible = requests.some((request) => !scope.dismissed.includes(identity(request)));
  const central =
    context.environment || requests.some((request) => identity(request) === scope.announced);
  return {
    origin: scope.origin,
    central: eligible && central,
    environment: eligible && !central,
    open: scope.open,
    openList: () => setScope((current) => ({ ...current, open: true })),
    closeList: () => setScope((current) => ({ ...current, open: false })),
    started: (request: DeletionRequest, origin: object) =>
      setScope((current) =>
        current.origin === origin ? { ...current, announced: identity(request) } : current,
      ),
    dismiss: () => {
      const dismissed = [...new Set([...scope.dismissed, ...requests.map(identity)])];
      try {
        sessionStorage.setItem(storageKey(viewerId), JSON.stringify(dismissed));
        fallback.delete(viewerId);
      } catch {
        fallback.set(viewerId, dismissed);
      }
      setScope((current) => ({ ...current, dismissed }));
    },
  };
}
