import type { BridgePlacement, ChannelUsageSummary } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { accessDeadline, loadPlacementHierarchy } from './placementAccess';

export type RetainedTarget = Extract<BridgePlacement, { kind: 'existing_agent' }>['target'];
export const retainedTargetComplete = (target: RetainedTarget) =>
  Object.values(target).every(Boolean);

/** Verify only the exact selected organization hierarchy; preserve public redaction. */
export async function loadRetainedTarget(
  client: BridgeViewsClient,
  target: RetainedTarget,
  organization: string,
  signal: AbortSignal,
) {
  const started = Date.now();
  const required = ['view_details', 'manage_bridge', 'interact'];
  const initial = await client.resourceAccess.effective('agent', target.agent_id, { signal });
  let until = accessDeadline(initial, organization, started, required);
  const hierarchy = await loadPlacementHierarchy(
    client,
    target.sandbox_id,
    target.workspace_id,
    organization,
    signal,
  );
  until = Math.min(until, Date.parse(hierarchy.revalidateAt));
  const { sandbox, workspace } = hierarchy.value;
  const row = workspace?.agents.find((agent) => agent.id === target.agent_id);
  const tree = workspace?.worktrees.find((worktree) => worktree.id === target.worktree_id);
  if (
    row === undefined ||
    tree === undefined ||
    workspace?.agents.filter((item) => item.id === target.agent_id).length !== 1 ||
    workspace?.worktrees.filter((item) => item.id === target.worktree_id).length !== 1 ||
    row.sandbox_id !== target.sandbox_id ||
    row.workspace_id !== target.workspace_id ||
    row.worktree_id !== target.worktree_id ||
    tree.agent_id !== target.agent_id ||
    tree.sandbox_id !== target.sandbox_id ||
    tree.workspace_id !== target.workspace_id ||
    tree.status !== 'ready'
  )
    throw new Error('The selected Agent no longer belongs to this exact hierarchy.');
  const agent = await client.agents.readAccessible(target.agent_id, { signal });
  if (agent.id !== target.agent_id || ['stopped', 'deleted', 'failed'].includes(agent.status))
    throw new Error('The selected Agent cannot accept work.');
  if (
    agent.access !== 'shared' &&
    (agent.sandbox.id !== target.sandbox_id ||
      agent.workspace.id !== target.workspace_id ||
      agent.worktree.id !== target.worktree_id)
  )
    throw new Error('The selected Agent hierarchy changed.');
  let channelUsage: ChannelUsageSummary | undefined;
  try {
    channelUsage = (await client.agents.channelUsage(target.agent_id, { signal })).channel_usage;
  } catch {
    signal.throwIfAborted();
  }
  const current = await client.resourceAccess.effective('agent', target.agent_id, { signal });
  until = Math.min(until, accessDeadline(current, organization, started, required));
  signal.throwIfAborted();
  if (initial.policy_revision !== current.policy_revision || until <= Date.now())
    throw new Error('Current Agent access changed. Review the selected target again.');
  return {
    value: {
      sandbox,
      workspace: workspace!,
      agent,
      model: agent.access === 'shared' ? agent.model : agent.profile.model,
      services: sandbox.access === 'shared' ? undefined : sandbox.nyxid_services,
      channelUsage,
    },
    revalidateAt: new Date(until).toISOString(),
  };
}
