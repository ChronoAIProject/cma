/*
 * Local Event Gateway fixtures.
 *
 * One source for both the `dev:fixtures` server (`build/devFixtures.ts`) and
 * the LOCAL channel-map browser check (`e2e/channel-map.check.ts`). They model
 * the public response shapes only; no gateway, provider or CMA backend runs.
 * Values are illustrative and must never resemble a credential.
 */

export const localOwner = { subject: 'local-owner', display_name: 'Local owner' } as const;

/** CMA browser sign-in (`GET /auth/session`). */
export const cmaSession = {
  authenticated: true,
  ...localOwner,
  capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox', 'manage_own_bots'],
};

/** Event Gateway sign-in (`GET /api/event-gateway/auth/session`). */
export const gatewaySession = {
  authenticated: true,
  subject: localOwner.subject,
  permissions: ['cma-eg:admin'],
  expires_at: 9999999999,
};

const platforms = ['telegram', 'whatsapp', 'lark', 'discord'] as const;

/** One channel with four relay producers and two consumers. */
export const teamChannel = {
  channel_id: 'channel-local',
  record_id: 'record-local',
  version: 3,
  created_at: '2026-10-27T01:00:00Z',
  owner: localOwner,
  provider: { slug: 'cma_codex', level: 'l2' },
  definition: {
    name: 'Team workspace',
    schema_version: 1,
    provider: { slug: 'cma_codex', binding_id: 'binding-local' },
    sources: platforms.map((platform) => ({
      type: 'nyxid_relay',
      platform,
      issuer: 'https://nyx.example',
      key_id: 'public-signing-key',
      route_ids: [`${platform}-team-route`],
      admission: { type: 'all' },
    })),
    partition: { version: 1, key: 'conversation_and_sender' },
    reply: { auto_final: true, ack_after_ms: 1500, sinks: [{ type: 'source' }] },
    routing: {
      version: 1,
      default_consumer: { name: 'assistant', label: 'CMA Agent' },
      default_targets: ['default'],
      consumers: [
        {
          id: 'reviewer',
          name: 'reviewer',
          label: 'Nyxbot reviewer',
          provider: { slug: 'nyxbot', binding_id: 'binding-reviewer' },
        },
      ],
      rules: [],
    },
  },
  endpoints: {
    custom_event_url: 'https://gateway.example/events',
    nyxid_callback_url: 'https://gateway.example/callback',
  },
};

const agentTools = [
  [
    'replyToEvent',
    'Send a reply to the event sender with the original destination and confirmed delivery receipt.',
  ],
  [
    'updateMessage',
    'Update a confirmed message when its transport supports edits and the saved source still authorizes the action.',
  ],
  ['sendTyping', 'Show a typing indicator when the original platform and conversation support it.'],
  [
    'readEventContext',
    'Read the incoming event, original sender and conversation, and the capabilities of its source.',
  ],
  [
    'readEventAttachment',
    'Read an authorized image, document, audio transcript or video attachment from the original event.',
  ],
  [
    'readMessageCapabilities',
    'Check the deletion and reaction capabilities of a confirmed message before attempting an action.',
  ],
  [
    'deleteMessage',
    'Delete an authorized confirmed bot message within the platform permission and age limits.',
  ],
  [
    'setMessageReaction',
    'Set or clear the bot’s own reaction on an authorized confirmed message without changing other reactions.',
  ],
  [
    'readEventHistory',
    'Read earlier messages in the original native Discord conversation with source conversation history consent.',
  ],
  ['prepareFormReply', 'Prepare a native WhatsApp form with explicit source consent.'],
] as const;

/** `GET /api/event-gateway/v1/channel-capabilities`. */
export const channelCapabilities = {
  agent_tools: agentTools.map(([name, description]) => ({
    name,
    description,
    read_only: name.startsWith('read'),
    destructive: ['updateMessage', 'deleteMessage', 'setMessageReaction'].includes(name),
    idempotent: name.startsWith('read'),
    open_world: true,
  })),
  relay_authoring: {
    dynamic_sources_writes_enabled: true,
    relay_raw_verification_writes_enabled: true,
    outbound_messages_writes_enabled: true,
    native_media_writes_enabled: true,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: true,
    thread_contract_writes_enabled: true,
    platforms: platforms.map((id) => ({
      id,
      label: id,
      verification_modes: [
        { id: 'raw_event', writes_enabled: true },
        { id: 'nyxid_normalized', writes_enabled: true },
      ],
    })),
  },
};

/** One `activity` frame of `GET .../channels/{id}/activity/stream`. */
export const channelActivity = {
  cursor: 'activity-fixture',
  retention_days: 7,
  limit: 50,
  has_more: false,
  items: [
    {
      event_id: 'event-fixture',
      source: { type: 'nyxid_relay', platform: 'telegram' },
      actor: { id: 'producer-fixture', display_name: 'Project updates', kind: 'service' },
      conversation: { id: 'team-chat', kind: 'group' },
      occurred_at: '2026-10-27T01:30:00Z',
      received_at: '2026-10-27T01:30:01Z',
      consumers: [
        {
          consumer_id: 'reviewer',
          display_name: 'Nyxbot reviewer',
          agent_id: 'agent-reviewer',
          provider: 'nyxbot',
          conversation_id: 'review-conversation',
          state: 'completed',
          dispatched_at: '2026-10-27T01:30:02Z',
          accepted_at: '2026-10-27T01:30:03Z',
          produced_at: '2026-10-27T01:30:04Z',
          completed_at: '2026-10-27T01:30:05Z',
          response_id: 'response-fixture',
          deliveries: [
            {
              step: 'answer',
              destination: 'origin',
              outcome: 'confirmed',
              observed_at: '2026-10-27T01:30:05Z',
              confirmed_at: '2026-10-27T01:30:05Z',
              platform_message_id: 'message-fixture',
              recipients: [{ kind: 'telegram_chat', id: 'team-chat', thread_id: null }],
            },
          ],
        },
      ],
    },
  ],
};

/** The SSE body carrying `channelActivity`, as the gateway frames it. */
export const channelActivityFrame = [
  `id: ${channelActivity.cursor}`,
  'event: activity',
  `data: ${JSON.stringify(channelActivity)}`,
  '',
  '',
].join('\n');
