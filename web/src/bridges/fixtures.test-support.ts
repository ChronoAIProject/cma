import {
  createClient,
  type BridgeConfigurationDetail,
  type BridgeConnectorChoices,
  type BridgeCreationTarget,
} from '@chronoai/cma-sdk';
import { vi } from 'vitest';
import { initialConfiguration } from './configuration';
import { NOW, WIRE_AGENT } from '../test/rest';

export const bridgeCreationTarget: BridgeCreationTarget = {
  bridge_id: `brg_${'a'.repeat(64)}`,
  owner: { kind: 'person', subject: 'owner' },
  callback_url: `https://cma.example/api/v1/bridge-callbacks/brg_${'a'.repeat(64)}`,
};

export const bridgeConfig = {
  ...initialConfiguration(),
  display_name: 'Client support',
  binding: {
    bot_id: 'bot-one',
    key_id: 'key-one',
    platform: 'telegram' as const,
    route_ids: ['route-one'],
  },
  partition: 'fixed_shared_agent' as const,
  placement: {
    kind: 'existing_agent' as const,
    target: {
      agent_id: WIRE_AGENT.id,
      sandbox_id: WIRE_AGENT.sandbox.id,
      workspace_id: WIRE_AGENT.workspace.id,
      worktree_id: WIRE_AGENT.worktree.id,
    },
  },
};
export const bridgeDetail: BridgeConfigurationDetail = {
  bridge: {
    id: 'brg_one',
    owner: { kind: 'person', subject: 'owner' },
    display_name: 'Client support',
    revision: 1,
    config_revision: 1,
    state: { kind: 'disabled', accepted_work: 'suspend_original' },
    emergency: { ingress_stopped: false, execution_stopped: false, outbound_stopped: false },
    bot_id: 'bot-one',
    platform: 'telegram',
    can_manage: true,
    created_at: NOW,
    updated_at: NOW,
  },
  config: bridgeConfig,
  callback_url: 'https://cma.example/api/v1/bridge-callbacks/brg_one',
  credential: { revision: 1, present: true, revoked: false },
};
export const bridgeChoices: BridgeConnectorChoices = {
  bots: [{ id: 'bot-one', platform: 'telegram', active: true }],
  keys: [{ id: 'key-one', active: true, callback_configured: true }],
  routes: [
    { id: 'route-one', bot_id: 'bot-one', key_id: 'key-one', platform: 'telegram', active: true },
  ],
};
export function bridgeTransport(
  answer?: (url: URL, init: RequestInit) => Response | Promise<Response> | undefined,
) {
  const fetch = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = new URL(String(input));
    const overridden = answer?.(url, init);
    if (overridden !== undefined) return overridden;
    if (url.pathname.endsWith('/bridge-creation-target')) {
      const organization = url.searchParams.get('organization');
      return Response.json({
        ...bridgeCreationTarget,
        owner:
          organization === null
            ? bridgeCreationTarget.owner
            : { kind: 'organization', subject: organization },
      });
    }
    if (url.pathname.endsWith('/invoke-readiness'))
      return Response.json({
        bridge_id: bridgeDetail.bridge.id,
        revision: 1,
        config_revision: 1,
        status: 'consent_required',
        candidate: null,
      });
    if (url.pathname.endsWith('/invoke-grants'))
      return Response.json({ items: [], next_cursor: null });
    if (url.pathname.endsWith('/organizations'))
      return Response.json({
        items: [{ id: 'org-one', role: 'member', display_name: 'Research' }],
      });
    if (url.pathname.endsWith('/bridge-connectors')) return Response.json(bridgeChoices);
    if (url.pathname.endsWith('/configuration')) return Response.json(bridgeDetail);
    if (url.pathname.endsWith('/bridges/brg_one')) return Response.json(bridgeDetail.bridge);
    if (url.pathname.endsWith('/bridges')) return Response.json({ items: [bridgeDetail.bridge] });
    if (url.pathname.endsWith(`/agents/${WIRE_AGENT.id}`)) return Response.json(WIRE_AGENT);
    if (url.pathname.endsWith('/agents'))
      return Response.json({
        items: [
          {
            id: WIRE_AGENT.id,
            sandbox_id: WIRE_AGENT.sandbox.id,
            workspace_id: WIRE_AGENT.workspace.id,
            worktree_id: WIRE_AGENT.worktree.id,
            status: WIRE_AGENT.status,
            profile: WIRE_AGENT.profile,
            title: WIRE_AGENT.title,
            created_at: NOW,
            updated_at: NOW,
          },
        ],
      });
    if (url.pathname.endsWith('-profiles')) return Response.json({ items: [] });
    throw new Error(`Unexpected fixture route ${url.pathname}`);
  });
  return { client: createClient({ baseUrl: 'https://fixture.invalid', fetch }), fetch };
}
