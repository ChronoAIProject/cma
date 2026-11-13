import { createContext, useContext, useMemo, type ReactNode } from 'react';
import type { AgentCreationCategory } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';

const CreationScope = createContext<ReturnType<typeof useCreationScope> | undefined>(undefined);

function useCreationScope(viewerId: string) {
  const client = useCmaClient();
  return useMemo(
    () => ({
      client,
      viewerId,
      // Store only immutable public categories inside this mounted account.
      facts: new Map<string, AgentCreationCategory>(),
    }),
    [client, viewerId],
  );
}

export function TreeCreationScope({
  viewerId,
  children,
}: {
  readonly viewerId: string;
  readonly children: ReactNode;
}) {
  const scope = useCreationScope(viewerId);
  return <CreationScope value={scope}>{children}</CreationScope>;
}

export function useTreeCreationScope(viewerId: string) {
  const shared = useContext(CreationScope);
  const local = useCreationScope(viewerId);
  return shared?.client === local.client && shared.viewerId === viewerId ? shared : local;
}
