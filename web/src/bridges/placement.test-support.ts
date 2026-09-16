import { bridgeTransport, bridgeDetail, bridgeCreationTarget } from './fixtures.test-support';
import {
  NOW,
  WIRE_SANDBOX_DETAIL,
  WIRE_WORKSPACE_DETAIL,
  WIRE_WORKSPACE_SUMMARY,
  wireProfile,
} from '../test/rest';

export const placementCatalog = {
  version: 1,
  source_revision: null,
  connection_validation_available: true,
  placements: [
    { kind: 'workspace_per_partition', availability: 'available' },
    { kind: 'worktree_agent_per_partition', availability: 'available' },
  ],
  connectors: [
    {
      platform: 'telegram',
      evidence: 'contract_tested',
      live_verified: false,
      inbound_text: true,
      stable_sender_chat: true,
      stable_thread: 'telegram_forum_topic',
      reply_to_bot: true,
      bot_mentions: false,
      inbound_edits: false,
      inbound_deletes: false,
      verified_actions: false,
      attachment_input: 'unavailable',
      attachment_output: false,
      output_text: true,
      output_edit: false,
      output_progress: true,
      output_limit_utf16: 4000,
      output_format: 'escaped_markdown',
      stages: {
        management: true,
        ingress: true,
        allocation: true,
        execution: true,
        outbound: true,
      },
    },
  ],
};

export function placementTransport(
  organization?: string,
  override?: (url: URL, init: RequestInit) => Response | Promise<Response> | undefined,
) {
  const writes: RequestInit[] = [];
  const test = bridgeTransport((url, init) => {
    const response = override?.(url, init);
    if (response !== undefined) return response;
    if (url.pathname.endsWith('/capabilities')) return Response.json(placementCatalog);
    if (url.pathname.includes('/resource-access/'))
      return Response.json({
        owner:
          organization === undefined
            ? { kind: 'person', is_current_person: true }
            : { kind: 'organization', organization_id: organization },
        capabilities: ['view_details', 'manage_sandbox'],
        revalidate_at: null,
        policy_revision: 1,
      });
    if (url.pathname.endsWith('/sandboxes')) return Response.json({ items: [WIRE_SANDBOX_DETAIL] });
    if (url.pathname.endsWith('/sandboxes/sandbox-1')) return Response.json(WIRE_SANDBOX_DETAIL);
    if (url.pathname.endsWith('/sandboxes/sandbox-1/workspaces'))
      return Response.json({ items: [WIRE_WORKSPACE_SUMMARY] });
    if (url.pathname.endsWith('/workspaces/workspace-1'))
      return Response.json(WIRE_WORKSPACE_DETAIL);
    for (const kind of ['agent', 'workspace', 'sandbox'] as const)
      if (url.pathname.endsWith(`/${kind}-profiles`))
        return Response.json({ items: [{ ...wireProfile(kind), visibility: 'public' }] });
    if (url.pathname.endsWith('/bridges') && init.method === 'POST') {
      writes.push(init);
      return Response.json(
        {
          bridge_id: bridgeCreationTarget.bridge_id,
          owner:
            organization === undefined
              ? { kind: 'person', subject: 'owner' }
              : { kind: 'organization', subject: organization },
          actor: { kind: 'human', subject: 'owner' },
          revision: 1,
          config_revision: 1,
          command: 'create',
          applied_at: NOW,
        },
        { status: 201 },
      );
    }
    return undefined;
  });
  return { ...test, writes };
}

export const savedShared = {
  ...bridgeDetail,
  config: {
    ...bridgeDetail.config,
    partition: 'conversation_sender' as const,
    placement: {
      kind: 'worktree_agent_per_partition' as const,
      sandbox_id: 'sandbox-1',
      workspace_id: 'workspace-1',
      agent_profile: { id: 'agent-1', revision: 7 },
      shared_trust: 'all_participants_share_filesystem_processes_and_credentials' as const,
      capacity: { keep_other_workspaces: 2, keep_other_agents_per_workspace: 3 },
    },
  },
};
