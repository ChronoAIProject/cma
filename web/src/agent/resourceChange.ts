import type { TreeAction } from '../shell/Tree';
import type { AgentResourceState } from './controllerTypes';

export type ResourceChange = Exclude<
  TreeAction,
  { kind: 'new_agent' | 'new_workspace' | 'export_history' | 'export_retained_conversation' }
>;

/** A resource mutation affects only its own Agent or descendants of its exact parent. */
export function affectsCurrentAgent(target: ResourceChange, state: AgentResourceState): boolean {
  if (state.status !== 'ready') return false;
  if ('agentId' in target) return target.agentId === state.detail.id;
  if ('workspaceId' in target) return target.workspaceId === state.detail.workspace.id;
  return target.sandboxId === state.detail.sandbox.id;
}
