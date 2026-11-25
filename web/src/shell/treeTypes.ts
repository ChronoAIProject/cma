import type { AppRoute } from '../routing/routes';
import type { AgentDetail } from '../api/types';
import type { RuntimeAttention } from './runtimeAttention';
import type { NavigationSandbox } from './navigation';
import type { CurrentTask } from './taskStatus';
import type { ResourceViewsClient } from '../resources/client';

export interface TreePermissions {
  readonly createWorkspace: boolean;
  readonly createAgent: boolean;
  readonly pauseSandbox: boolean;
  readonly deleteSandbox: boolean;
  readonly removeWorkspace: boolean;
  readonly stopAgent: boolean;
  readonly deleteAgent: boolean;
  readonly exportHistory: boolean;
}

export type TreeAction =
  | { readonly kind: 'new_workspace'; readonly sandboxId: string }
  | { readonly kind: 'pause_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'revive_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'delete_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'new_agent'; readonly workspaceId: string }
  | { readonly kind: 'new_agent'; readonly sandboxId: string }
  | { readonly kind: 'remove_workspace'; readonly workspaceId: string }
  | { readonly kind: 'stop_agent'; readonly agentId: string }
  | { readonly kind: 'delete_agent'; readonly agentId: string }
  | { readonly kind: 'export_history'; readonly agentId: string }
  | { readonly kind: 'export_retained_conversation'; readonly agentId: string };

export interface TreeProps {
  readonly currentTask?: CurrentTask;
  readonly selectedRetired?: AgentDetail;
  readonly onShowAll?: () => void;
  readonly attention?: RuntimeAttention;
  readonly showDeletionProgress?: boolean;
  readonly active?: boolean;
  readonly resourceClient?: ResourceViewsClient;
  readonly viewerId: string;
  readonly view?: 'agent' | 'sandbox';
  readonly statusFilter?: string;
  readonly items: readonly NavigationSandbox[];
  readonly refreshError?: string;
  readonly lifecycleBusy?: boolean;
  readonly busySandboxIds?: ReadonlySet<string>;
  readonly deletingSandboxIds?: ReadonlySet<string>;
  readonly onViewDeletion?: (sandboxId: string) => void;
  readonly route: AppRoute;
  readonly permissions: TreePermissions;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAgentIntent?: (agentId: string, active: boolean) => void;
  readonly onAction: (action: TreeAction) => void;
}
