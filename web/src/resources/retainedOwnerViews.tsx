import { createContext, useContext, useMemo, type ReactNode } from 'react';
import type { AgentCreationCategory } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import { useCmaClient } from '../api/context';
import type {
  AgentDetail,
  SandboxDetail,
  SandboxProfileSettings,
  WorkspaceDetail,
} from '../api/types';

class RecentViews<Value> {
  private readonly values = new Map<string, Value>();
  get(id: string) {
    return this.values.get(id);
  }
  delete(id: string) {
    this.values.delete(id);
  }
  put(id: string, value: Value) {
    this.values.delete(id);
    this.values.set(id, value);
    if (this.values.size > 6) this.values.delete(this.values.keys().next().value!);
  }
}

export interface SandboxContents {
  readonly workspaces: readonly WorkspaceDetail[];
  readonly agents: readonly AgentDetail[];
  readonly failed: boolean;
}

/** Display snapshots only: this cache never grants access or command authority. */
export class OwnerViews {
  readonly sandboxes = new RecentViews<{ sandbox: SandboxDetail; observedAt: string }>();
  readonly contents = new RecentViews<SandboxContents>();
  readonly settings = new RecentViews<{ profileKey: string; value: SandboxProfileSettings }>();
  readonly origins = new RecentViews<AgentCreationCategory>();
  constructor(
    readonly client: CmaWebClient,
    readonly viewerId: string,
  ) {}
  forgetSandbox(id: string) {
    this.sandboxes.delete(id);
    this.contents.delete(id);
    this.settings.delete(id);
  }
}

const RetainedOwnerViews = createContext<OwnerViews | undefined>(undefined);

export function OwnerViewsProvider({
  viewerId,
  children,
}: {
  readonly viewerId: string;
  readonly children: ReactNode;
}) {
  const client = useCmaClient();
  const views = useMemo(() => new OwnerViews(client, viewerId), [client, viewerId]);
  return <RetainedOwnerViews value={views}>{children}</RetainedOwnerViews>;
}

export function useOwnerViews() {
  const client = useCmaClient();
  const views = useContext(RetainedOwnerViews);
  return views?.client === client ? views : undefined;
}
