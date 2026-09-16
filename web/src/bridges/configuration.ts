import type { BridgeConfiguration, BridgePartitionPolicy } from '@chronoai/cma-sdk';
import { isShared, type SharedPlacement } from './placementAccess';

export const partitions: readonly {
  value: BridgePartitionPolicy;
  label: string;
  explanation: string;
}[] = [
  {
    value: 'conversation_sender',
    label: 'Each sender in each chat',
    explanation: 'Each sender gets a separate conversation and Sandbox in each chat.',
  },
  {
    value: 'conversation',
    label: 'Everyone in one chat',
    explanation: 'Everyone allowed in a chat shares its Agent conversation and Sandbox.',
  },
  {
    value: 'thread_sender',
    label: 'Each sender in each thread',
    explanation:
      'Each sender gets separate context per verified thread. Messages without a verified thread are refused.',
  },
  {
    value: 'thread',
    label: 'Everyone in one thread',
    explanation:
      'Allowed senders in a verified thread share its context. Messages without a verified thread are refused.',
  },
  {
    value: 'sender_across_chats',
    label: 'Each sender across chats',
    explanation:
      'A sender shares their conversation across chats. Replies may carry information learned in another chat.',
  },
  {
    value: 'fixed_shared_agent',
    label: 'One retained Agent for everyone',
    explanation:
      'Every allowed sender shares the selected Agent’s existing history, files and execution context.',
  },
];

export function initialConfiguration(): BridgeConfiguration {
  return {
    display_name: '',
    binding: { bot_id: '', platform: 'telegram', key_id: '', route_ids: [] },
    partition: 'conversation_sender',
    placement: {
      kind: 'new_sandbox',
      sandbox_profile: { id: '', revision: 1 },
      workspace_profile: { id: '', revision: 1 },
      agent_profile: { id: '', revision: 1 },
    },
    admission: {
      kind: 'scoped',
      policy: { senders: { kind: 'deny' }, chats: { kind: 'open' }, groups: 'deny' },
    },
    reply: 'final_response',
    limits: {
      max_partitions: 10,
      max_sandboxes: 10,
      max_queued_messages: 100,
      max_messages_per_minute: 20,
      max_payload_bytes: 65536,
      inbox_retention_seconds: 86400,
      reply_secret_retention_seconds: 1800,
    },
  };
}

export function configurationComplete(config: BridgeConfiguration): boolean {
  const p = config.placement;
  const pin = (value: { id: string; revision: number }) =>
    value.id.trim() !== '' && Number.isSafeInteger(value.revision) && value.revision > 0;
  const reserve = (value: number) => Number.isInteger(value) && value >= 1 && value <= 15;
  const placementComplete = isShared(p)
    ? p.sandbox_id.trim() !== '' &&
      (p.kind === 'workspace_per_partition'
        ? pin(p.workspace_profile)
        : p.workspace_id.trim() !== '') &&
      pin(p.agent_profile) &&
      p.shared_trust === 'all_participants_share_filesystem_processes_and_credentials' &&
      reserve(p.capacity.keep_other_workspaces) &&
      reserve(p.capacity.keep_other_agents_per_workspace) &&
      config.partition !== 'fixed_shared_agent'
    : p.kind === 'existing_agent'
      ? Object.values(p.target).every((id) => id.trim() !== '')
      : pin(p.agent_profile) && pin(p.workspace_profile) && pin(p.sandbox_profile);
  return (
    config.display_name.trim() !== '' &&
    config.binding.bot_id !== '' &&
    config.binding.key_id !== '' &&
    config.binding.route_ids.length > 0 &&
    placementComplete
  );
}

export function initialSharedPlacement(kind: SharedPlacement['kind']): SharedPlacement {
  const fields = {
    sandbox_id: '',
    agent_profile: { id: '', revision: 1 },
    shared_trust: 'all_participants_share_filesystem_processes_and_credentials' as const,
    capacity: { keep_other_workspaces: 1, keep_other_agents_per_workspace: 1 },
  };
  // The editor keeps a separate unchecked acknowledgment and cannot submit this
  // draft until the user explicitly consents to its exact selected placement.
  return kind === 'workspace_per_partition'
    ? { kind, ...fields, workspace_profile: { id: '', revision: 1 } }
    : { kind, ...fields, workspace_id: '' };
}
