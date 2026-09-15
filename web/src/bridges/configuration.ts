import type { BridgeConfiguration, BridgePartitionPolicy } from '@chronoai/cma-sdk';

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
  return (
    config.display_name.trim() !== '' &&
    config.binding.bot_id !== '' &&
    config.binding.key_id !== '' &&
    config.binding.route_ids.length > 0 &&
    (p.kind === 'existing_agent'
      ? Object.values(p.target).every(Boolean)
      : p.kind === 'new_sandbox' &&
        p.agent_profile.id !== '' &&
        p.workspace_profile.id !== '' &&
        p.sandbox_profile.id !== '')
  );
}
