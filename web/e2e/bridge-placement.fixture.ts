import type { Page } from '@playwright/test';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import {
  organizationAccess,
  organizationResponse,
  organizationTime,
} from '../src/bridges/organization.fixture';

/** Scripted public boundary only; no backend, provider, Mongo or runtime effects. */
export async function placementBoundary(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  const state = {
    available: true,
    authorized: true,
    loseFirstSave: false,
    writes: [] as { body: Record<string, unknown>; key: string | undefined }[],
  };
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const reply = (json: unknown, status = 200) => route.fulfill({ json, status });
    if (url.pathname === '/api/v1/organizations')
      return reply({
        items: [{ id: 'org-one', role: 'admin', display_name: 'Research organization' }],
      });
    if (url.pathname === '/api/v1/bridge-connectors')
      return reply({
        bots: [{ id: 'bot-one', platform: 'telegram', active: true }],
        keys: [{ id: 'key-one', active: true, callback_configured: true }],
        routes: [
          {
            id: 'route-one',
            bot_id: 'bot-one',
            key_id: 'key-one',
            platform: 'telegram',
            active: true,
          },
        ],
      });
    if (url.pathname === '/api/v1/bridge-connectors/capabilities')
      return reply({
        version: 1,
        source_revision: null,
        connection_validation_available: true,
        placements: ['workspace_per_partition', 'worktree_agent_per_partition'].map((kind) => ({
          kind,
          availability: state.available ? 'available' : 'continuity_unverified',
        })),
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
      });
    if (url.pathname.includes('/resource-access/'))
      return state.authorized
        ? reply(organizationAccess)
        : reply({ code: 'resource_forbidden' }, 403);
    if (url.pathname === '/api/v1/bridges' && request.method() === 'POST') {
      state.writes.push({
        body: request.postDataJSON() as Record<string, unknown>,
        key: request.headers()['idempotency-key'],
      });
      if (state.loseFirstSave && state.writes.length === 1) return route.abort('connectionfailed');
      return reply(
        {
          bridge_id: 'brg_created',
          owner: { kind: 'organization', subject: 'org-one' },
          actor: { kind: 'human', subject: 'local-owner' },
          revision: 1,
          config_revision: 1,
          command: 'create',
          applied_at: organizationTime,
        },
        201,
      );
    }
    if (url.pathname === '/api/v1/bridges/brg_created') return reply({ code: 'not_found' }, 404);
    const resource = organizationResponse(url);
    if (resource !== undefined) return reply(resource);
    const profile = /^\/api\/v1\/(sandbox|workspace|agent)-profiles$/.exec(url.pathname);
    if (profile !== null) {
      const kind = profile[1]!;
      return reply({
        items: [
          {
            ...recorded<Record<string, unknown>>('profiles.json', `${kind}-profile-read`),
            id: `${kind}-1`,
            name: `Research ${kind}`,
            visibility: 'public',
            latest_revision: {
              revision: 1,
              digest: 'public-fixture-revision',
              published_at: organizationTime,
            },
          },
        ],
      });
    }
    return route.fallback();
  });
  return { boundary, state };
}
