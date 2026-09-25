import { useCallback, useMemo, useState } from 'react';
import { routePath, type AppRoute } from '../routing/routes';
import type { ResourceViewsClient } from './client';
import { scopeValue } from './scope';
import { useAuthorizedRead } from './useAuthorizedRead';

/** Reloads derive resource scope from current authority, never from browser storage. */
export function useConsoleScope(
  client: ResourceViewsClient | undefined,
  viewer: string,
  route: AppRoute,
) {
  const context = useMemo(() => ({ client, viewer }), [client, viewer]);
  const path = routePath(route);
  const inventory =
    route.kind === 'inventory' ? scopeValue(route.organization, route.shared) : undefined;
  const targetKind =
    route.kind === 'agent' || route.kind === 'workspace' || route.kind === 'sandbox'
      ? route.kind
      : undefined;
  const targetId =
    route.kind === 'agent'
      ? route.agentId
      : route.kind === 'workspace'
        ? route.workspaceId
        : route.kind === 'sandbox'
          ? route.sandboxId
          : undefined;
  const [selection, setSelection] = useState({
    context,
    path,
    value: inventory ?? '',
    explicit: false,
  });
  if (selection.context !== context || selection.path !== path) {
    setSelection({
      context,
      path,
      value: inventory ?? (selection.context === context ? selection.value : ''),
      explicit: false,
    });
  }
  const read = useMemo(() => {
    const scopedClient = context.client;
    return scopedClient === undefined ||
      targetKind === undefined ||
      targetId === undefined ||
      selection.explicit
      ? undefined
      : async (signal: AbortSignal) => {
          const access = await scopedClient.resourceAccess.effective(targetKind, targetId, {
            signal,
          });
          if (!access.capabilities.includes('view_details'))
            throw new Error('Current resource access is unavailable.');
          const value =
            access.owner?.kind === 'organization'
              ? scopeValue(access.owner.organization_id)
              : access.owner?.kind === 'person'
                ? access.owner.is_current_person
                  ? ''
                  : 'shared'
                : undefined;
          return { value, revalidateAt: access.revalidate_at };
        };
  }, [context, targetKind, targetId, selection.explicit]);
  const current = useAuthorizedRead(read);
  const resolved = current.state.status === 'ready' ? current.state.value : undefined;
  if (
    resolved !== undefined &&
    selection.context === context &&
    selection.path === path &&
    !selection.explicit &&
    selection.value !== resolved
  )
    setSelection({ ...selection, value: resolved });
  const select = useCallback(
    (value: string) => setSelection({ context, path, value, explicit: true }),
    [context, path],
  );
  return {
    value: !selection.explicit && resolved !== undefined ? resolved : selection.value,
    select,
    ready:
      client === undefined ||
      targetKind === undefined ||
      selection.explicit ||
      (current.state.status === 'ready' && resolved !== undefined),
  };
}
