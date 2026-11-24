// @vitest-environment node
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';
import {
  EventWorkerProfileSchema,
  NativeMediaCapabilitiesSchema,
  ActivityRoutingSchema,
  ActivitySchema,
  AttachmentSchema,
  AttachmentSourceSchema,
  ChannelResponseSchema,
  ChannelPageSchema,
  ConsumerSummarySchema,
  ContextResponseSchema,
  EventContextSnapshotSchema,
  PublicActivitySchema,
  RouteResolutionSchema,
  RelayAuthoringCapabilitiesSchema,
  CustomAttachmentsSchema,
  DefaultReplySchema,
  RoutingPolicySchema,
  SourceRequestSchema,
  ThreadListSchema,
  ThreadReceiptSchema,
  ThreadCapabilitiesSchema,
  TopicLifecycleCapabilitiesSchema,
  ConversationHistoryPageSchema,
  ConversationHistoryCapabilitiesSchema,
  CreateThreadRequestSchema,
  ManageThreadRequestSchema,
  SourceSchema,
} from '../cmaeg/types.js';

it('preserves the opaque verified sender provider policy', () => {
  const fixtures = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, Record<string, unknown>[]>;
  for (const enabled of [true, false]) {
    const profile = {
      ...fixtures.EventWorkerProfile![0],
      environment: { verified_sender_context: enabled },
    };
    expect(EventWorkerProfileSchema(profile)).toEqual(profile);
  }
});

it('preserves older discovery and optional provider recovery and WhatsApp capabilities', () => {
  const legacy = {
    dynamic_sources_writes_enabled: true,
    relay_raw_verification_writes_enabled: true,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  };
  expect(RelayAuthoringCapabilitiesSchema(legacy)).toEqual(legacy);
  for (const providers of [null, [], ['cma_codex', 'nyxbot']]) {
    const value = { ...legacy, response_recovery_providers: providers };
    expect(RelayAuthoringCapabilitiesSchema(value)).toEqual(value);
  }
  for (const enabled of [false, true, null]) {
    const value = {
      ...legacy,
      native_whatsapp_writes_enabled: enabled,
      native_whatsapp_auto_activation_enabled: enabled,
    };
    expect(RelayAuthoringCapabilitiesSchema(value)).toEqual(value);
  }
});

it('does not infer General topics from the numbered topic gate on older servers', () => {
  const legacy = {
    dynamic_sources_writes_enabled: true,
    relay_raw_verification_writes_enabled: true,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    native_telegram_topic_lifecycle_enabled: true,
    platforms: [],
  };
  expect(RelayAuthoringCapabilitiesSchema(legacy)).toEqual(legacy);
  for (const enabled of [false, true, null]) {
    const value = { ...legacy, native_telegram_general_topic_lifecycle_enabled: enabled };
    expect(RelayAuthoringCapabilitiesSchema(value)).toEqual(value);
  }
});

interface Schema {
  readonly $ref?: string;
  readonly type?: string | readonly string[];
  readonly properties?: Readonly<Record<string, Schema>>;
  readonly required?: readonly string[];
  readonly enum?: readonly string[];
  readonly items?: Schema;
  readonly oneOf?: readonly Schema[];
  readonly allOf?: readonly Schema[];
  readonly additionalProperties?: boolean | Schema;
}
const snapshot = JSON.parse(
  readFileSync(new URL('../../../../api/crates/cmaeg/openapi/v1.json', import.meta.url), 'utf8'),
) as {
  components: { schemas: Record<string, Schema> };
};

function typeOf(schema: Schema): string {
  if (schema.$ref !== undefined) return `Expected${schema.$ref.split('/').at(-1) ?? ''}`;
  if (schema.oneOf !== undefined) return `(${schema.oneOf.map(typeOf).join(' | ')})`;
  if (schema.allOf !== undefined) return `(${schema.allOf.map(typeOf).join(' & ')})`;
  if (Array.isArray(schema.type))
    return `(${schema.type.map((type: string) => typeOf({ ...schema, type })).join(' | ')})`;
  if (schema.enum !== undefined)
    return schema.enum.map((value) => JSON.stringify(value)).join(' | ');
  if (schema.type === 'array') return `readonly (${typeOf(schema.items ?? {})})[]`;
  if (schema.type === 'object') {
    const fields = Object.entries(schema.properties ?? {}).map(
      ([name, value]) =>
        `readonly ${JSON.stringify(name)}${schema.required?.includes(name) === true ? '' : '?'}: ${typeOf(value)}`,
    );
    if (schema.additionalProperties !== undefined && schema.additionalProperties !== false)
      fields.push(
        `readonly [key: string]: ${typeOf(schema.additionalProperties === true ? {} : schema.additionalProperties)}`,
      );
    return `{${fields.join(';')}}`;
  }
  if (schema.type === 'integer' || schema.type === 'number') return 'number';
  if (schema.type === 'string' || schema.type === 'boolean' || schema.type === 'null')
    return schema.type;
  return 'unknown';
}

function check(schemas: Readonly<Record<string, Schema>>): readonly ts.Diagnostic[] {
  const directory = mkdtempSync(join(tmpdir(), 'cmaeg-types-'));
  try {
    const path = join(directory, 'contract.mts');
    const source = fileURLToPath(new URL('../cmaeg/types.js', import.meta.url));
    writeFileSync(
      path,
      [
        `import type * as Sdk from ${JSON.stringify(source)};`,
        'type Normalize<T> = T extends readonly (infer Item)[] ? readonly Normalize<Item>[] : T extends object ? { readonly [K in keyof T]: Normalize<T[K]> } : T;',
        'type Equal<A,B> = (<T>()=>T extends A?1:2) extends (<T>()=>T extends B?1:2) ? true : false;',
        'type Assert<T extends true> = T;',
        ...Object.entries(schemas).map(
          ([name, schema]) => `type Expected${name} = ${typeOf(schema)};`,
        ),
        ...Object.keys(schemas).map(
          (name) =>
            `export type Check${name} = Assert<Equal<Normalize<Sdk.${name}>,Normalize<Expected${name}>>>;`,
        ),
      ].join('\n'),
    );
    const program = ts.createProgram([path], {
      strict: true,
      noEmit: true,
      skipLibCheck: true,
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.NodeNext,
      moduleResolution: ts.ModuleResolutionKind.NodeNext,
    });
    return ts.getPreEmitDiagnostics(program);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe('CMAEG schema inventory', () => {
  it('matches every snapshot type, property, enum, nullability and required field', () => {
    const diagnostics = check(snapshot.components.schemas);
    expect(
      diagnostics.map((value) => {
        const line = value.file?.getLineAndCharacterOfPosition(value.start ?? 0).line;
        const declaration = line === undefined ? '' : value.file?.text.split('\n')[line];
        return `${declaration ?? ''}: ${ts.flattenDiagnosticMessageText(value.messageText, '\n')}`;
      }),
    ).toEqual([]);
  }, 30_000);

  it('fails when a snapshot response property is removed', () => {
    const schemas = structuredClone(snapshot.components.schemas);
    const response = schemas.ContextResponse;
    if (response?.properties === undefined) throw new Error('missing context schema');
    const properties = { ...response.properties };
    delete properties.capabilities;
    schemas.ContextResponse = { ...response, properties };
    expect(check(schemas).some((value) => value.code === 2344)).toBe(true);
  }, 30_000);
});

describe('CMAEG attachment locators', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, readonly unknown[]>;

  it('parses every locator type and media fact from the contract examples', () => {
    const locators = examples.AttachmentSource ?? [];
    expect(locators.map((value) => (value as { type: string }).type)).toEqual([
      'nyxid_relay',
      'telegram',
      'url',
      'inline',
      'discord',
      'lark_resource',
      'whatsapp',
      'native_lark',
    ]);
    for (const example of locators) expect(AttachmentSourceSchema(example)).toEqual(example);
    for (const example of examples.Attachment ?? [])
      expect(AttachmentSchema(example)).toEqual(example);
  });

  it('refuses an unknown locator type and a malformed media fact', () => {
    expect(() => AttachmentSourceSchema({ type: 'lark', key: 'fixture' })).toThrow(
      'unknown type discriminator',
    );
    expect(() =>
      AttachmentSchema({ reference: 'fixture', kind: 'file', variant: 'hologram' }),
    ).toThrow('variant');
  });
});

it.each(['telegram', 'lark', 'feishu', 'discord', 'whatsapp', 'slack'])(
  'preserves a legacy %s relay without introducing a resource binding',
  (platform) => {
    const request = {
      type: 'nyxid_relay',
      platform,
      issuer: 'https://nyx.example',
      key_id: 'key-fixture',
      route_ids: ['route-fixture'],
      admission: { type: 'all' },
    };
    expect(SourceRequestSchema(request)).toEqual(request);
    expect(JSON.stringify(SourceRequestSchema(request))).not.toContain('lark_resources');
    const source = { ...request, jwks_url: 'https://nyx.example/.well-known/jwks.json' };
    expect(SourceSchema(source)).toEqual(source);
    expect(JSON.stringify(SourceSchema(source))).not.toContain('lark_resources');
  },
);

describe('CMAEG custom source attachments', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, readonly unknown[]>;
  const custom = (name: string) =>
    (examples[name] ?? []).filter((value) => (value as { type: string }).type === 'custom');

  it('round-trips the setting alone and on both custom source shapes', () => {
    for (const example of examples.CustomAttachments ?? [])
      expect(CustomAttachmentsSchema(example)).toEqual(example);
    expect(custom('Source')).toHaveLength(1);
    expect(custom('SourceRequest')).toHaveLength(1);
    for (const example of custom('Source')) expect(SourceSchema(example)).toEqual(example);
    for (const example of custom('SourceRequest'))
      expect(SourceRequestSchema(example)).toEqual(example);
  });

  it('accepts an absent or null setting and refuses a malformed one', () => {
    const base = { type: 'custom', auth: { type: 'hmac', key_id: 'fixture' } };
    expect(SourceRequestSchema(base)).toEqual(base);
    expect(SourceRequestSchema({ ...base, attachments: null })).toEqual({
      ...base,
      attachments: null,
    });
    expect(CustomAttachmentsSchema({})).toEqual({});
    expect(() =>
      SourceRequestSchema({ ...base, attachments: { origins: 'https://fixture.example' } }),
    ).toThrow('attachments: origins');
    expect(() => SourceSchema({ ...base, attachments: { pointer: 7 } })).toThrow(
      'attachments: pointer',
    );
  });
});

describe('CMAEG routing projection', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, readonly unknown[]>;

  it('round-trips every routed channel, policy and default reply variant', () => {
    const responses = examples.ChannelResponse ?? [];
    expect(responses.length).toBeGreaterThan(1);
    for (const example of responses) expect(ChannelResponseSchema(example)).toEqual(example);
    for (const example of examples.RoutingPolicy ?? [])
      expect(RoutingPolicySchema(example)).toEqual(example);
    const replies = (examples.DefaultReply ?? []).map((value) => DefaultReplySchema(value).type);
    expect(replies).toEqual(['telegram_chat', 'relay_conversation', 'sinks']);
  });

  it('reads a channel without routing exactly as before', () => {
    const routed = ChannelResponseSchema(examples.ChannelResponse?.[0]);
    expect(routed.definition.routing?.consumers?.[0]?.agent_key_hash).toBe('fixture');
    const { routing, ...definition } = routed.definition;
    expect(routing).toBeDefined();
    const legacy = { ...routed, definition };
    const parsed = ChannelResponseSchema(legacy);
    expect(parsed).toEqual(legacy);
    expect('routing' in parsed.definition).toBe(false);
  });

  it('refuses a malformed routing policy', () => {
    const policy = examples.RoutingPolicy?.[0] as Record<string, unknown>;
    expect(() => RoutingPolicySchema({ ...policy, reply_label: 'sometimes' })).toThrow(
      'reply_label',
    );
    expect(() => DefaultReplySchema({ type: 'email' })).toThrow('unknown type discriminator');
  });
});

describe('CMAEG activity routing', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, readonly unknown[]>;
  const first = (name: string) => examples[name]?.[0] as Record<string, unknown>;

  it('round-trips the routed activity, its projections and the routing components', () => {
    expect(ActivitySchema(first('Activity'))).toEqual(first('Activity'));
    expect(PublicActivitySchema(first('PublicActivity'))).toEqual(first('PublicActivity'));
    expect(ContextResponseSchema(first('ContextResponse'))).toEqual(first('ContextResponse'));
    expect(EventContextSnapshotSchema(first('EventContextSnapshot'))).toEqual(
      first('EventContextSnapshot'),
    );
    for (const example of examples.ActivityRouting ?? [])
      expect(ActivityRoutingSchema(example)).toEqual(example);
    expect((examples.RouteResolution ?? []).map((value) => RouteResolutionSchema(value))).toEqual([
      'default',
      'rule',
      'mention',
      'command',
      'event_field',
      'interaction',
      'agent_event',
    ]);
    const routing = ActivitySchema(first('Activity')).routing;
    expect(routing?.targets[0]?.id).toBe('csm_00000000000a');
    expect(routing?.causation?.hop).toBe(2);
  });

  it('reads an activity without routing exactly as before', () => {
    const { routing, ...legacy } = first('Activity');
    expect(routing).toBeDefined();
    const parsed = ActivitySchema(legacy);
    expect(parsed).toEqual(legacy);
    expect('routing' in parsed).toBe(false);
    const context = first('ContextResponse');
    const { routing: projected, ...activity } = context.activity as Record<string, unknown>;
    expect(projected).toBeDefined();
    const legacyContext = { ...context, activity };
    const parsedContext = ContextResponseSchema(legacyContext);
    expect(parsedContext).toEqual(legacyContext);
    expect('routing' in parsedContext.activity).toBe(false);
  });

  it('refuses a malformed routing decision', () => {
    const routing = first('ActivityRouting');
    expect(() => ActivityRoutingSchema({ ...routing, resolution: 'broadcast' })).toThrow(
      'resolution',
    );
    expect(() => ActivityRoutingSchema({ ...routing, targets: 'reviewer' })).toThrow('targets');
  });
});

describe('CMAEG context consumer', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, readonly unknown[]>;
  const context = (index: number) => examples.ContextResponse?.[index] as Record<string, unknown>;

  it('names the calling consumer of a routed channel and none on a hand-off', () => {
    for (const example of examples.ConsumerSummary ?? [])
      expect(ConsumerSummarySchema(example)).toEqual(example);
    const routed = ContextResponseSchema(context(0));
    expect(routed).toEqual(context(0));
    expect(routed.consumer).toEqual({
      id: 'csm_00000000000a',
      label: 'Reviewer',
      name: 'reviewer',
    });
    const handoff = ContextResponseSchema(context(1));
    expect([handoff.capabilities.buttons, handoff.capabilities.cards]).toEqual(['none', 'none']);
  });

  it('reads an unrouted context response exactly as before', () => {
    const { consumer, ...legacy } = context(0);
    expect(consumer).toBeDefined();
    const parsed = ContextResponseSchema(legacy);
    expect(parsed).toEqual(legacy);
    expect('consumer' in parsed).toBe(false);
    expect(() => ContextResponseSchema({ ...legacy, consumer: { id: 'default' } })).toThrow(
      'consumer',
    );
  });
});

it('keeps legacy listed records without inventing channel IDs or endpoints', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, readonly unknown[]>;
  const page = ChannelPageSchema(examples.ChannelPage?.[0]);
  const original = page.items[0];
  expect(original).toBeDefined();
  if (original === undefined) throw new Error('missing list fixture');
  const { channel_id, endpoints, ...record } = original;
  expect(channel_id).toBeDefined();
  expect(endpoints).toBeDefined();
  const legacy = { ...page, items: [record] };
  expect(ChannelPageSchema(legacy)).toEqual(legacy);
});

it('retains native sources without adding thread history access', () => {
  const value = { type: 'discord', bot_id: '77', intents: 37377, admission: { type: 'all' } };
  expect(SourceSchema(value)).toEqual(value);
  expect(SourceRequestSchema(value)).toEqual(value);
  expect(JSON.stringify(SourceSchema(value))).not.toContain('thread_history');
});

it('preserves legacy native media capability omissions and exact native WhatsApp limits', () => {
  const legacy = {
    kinds: ['image', 'file', 'audio', 'video'],
    sources: ['url', 'base64'],
    max_attachments: 4,
    max_inline_bytes: 65536,
  };
  expect(NativeMediaCapabilitiesSchema(legacy)).toEqual(legacy);
  const native = {
    ...legacy,
    kinds: ['image', 'file', 'audio', 'sticker', 'video'],
    mime_types: [
      'image/jpeg',
      'image/png',
      'application/pdf',
      'audio/aac',
      'audio/amr',
      'audio/mpeg',
      'audio/mp4',
      'audio/ogg',
      'image/webp',
      'video/mp4',
      'video/3gpp',
    ],
    max_total_bytes: 8388608,
    max_image_bytes: 5000000,
    max_caption_chars: 1024,
    video: {
      video_codec: 'h264',
      video_profile: 'baseline',
      audio_codec: 'aac_lc',
      max_audio_tracks: 1,
      max_width: 1920,
      max_height: 1080,
      max_frame_rate: 60,
      max_duration_ms: 600000,
      max_samples: 65536,
      progressive_only: true,
      max_slices_per_frame: 1,
      fast_start_required: true,
    },
    sticker: {
      width: 512,
      height: 512,
      max_static_bytes: 100000,
      max_animated_bytes: 500000,
      max_frames: 128,
      max_duration_ms: 10000,
      min_frame_duration_ms: 8,
    },
  };
  expect(NativeMediaCapabilitiesSchema(native)).toEqual(native);
});

it('round-trips bounded thread lifecycle contracts and preserves absent consent', () => {
  const fixtures = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, unknown[]>;
  for (const [name, schema] of [
    ['ThreadList', ThreadListSchema],
    ['ThreadReceipt', ThreadReceiptSchema],
    ['ThreadCapabilities', ThreadCapabilitiesSchema],
    ['CreateThreadRequest', CreateThreadRequestSchema],
    ['ManageThreadRequest', ManageThreadRequestSchema],
  ] as const) {
    for (const value of fixtures[name] ?? []) expect(schema(value)).toEqual(value);
  }
  const legacy = { type: 'discord', bot_id: '77', intents: 37377, admission: { type: 'all' } };
  expect(SourceSchema(legacy)).toEqual(legacy);
  expect(SourceRequestSchema(legacy)).toEqual(legacy);
  expect(SourceSchema({ ...legacy, thread_lifecycle: true })).toEqual({
    ...legacy,
    thread_lifecycle: true,
  });
});

it('requires a rename name and rejects it on every existing thread action', () => {
  const target = { type: 'event', event_ref: 'cmaeg1.ev.fixture' };
  const rename = { target, action: 'rename', name: 'Incident triage' };
  expect(ManageThreadRequestSchema(rename)).toEqual(rename);
  expect(() => ManageThreadRequestSchema({ target, action: 'rename' })).toThrow();
  expect(() => ManageThreadRequestSchema({ target, action: 'rename', name: null })).toThrow();
  for (const action of ['archive', 'reopen', 'join', 'leave']) {
    const legacy = { target, action };
    expect(ManageThreadRequestSchema(legacy)).toEqual(legacy);
    expect(() => ManageThreadRequestSchema({ ...legacy, name: 'Unexpected' })).toThrow();
    expect(() => ManageThreadRequestSchema({ ...legacy, name: null })).toThrow();
  }
});

it('round-trips paginated history facts and keeps consent independent', () => {
  const fixtures = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, unknown[]>;
  for (const value of fixtures.ConversationHistoryPage ?? [])
    expect(ConversationHistoryPageSchema(value)).toEqual(value);
  for (const value of fixtures.ConversationHistoryCapabilities ?? [])
    expect(ConversationHistoryCapabilitiesSchema(value)).toEqual(value);
  const source = {
    type: 'discord',
    bot_id: '77',
    intents: 37377,
    admission: { type: 'all' },
    conversation_history: true,
  };
  expect(SourceSchema(source)).toEqual(source);
  expect(SourceRequestSchema(source)).toEqual(source);
});

it('keeps native Lark history consent optional and accepts bounded empty thread progress without new page fields', () => {
  const source = {
    type: 'native_lark',
    separate_app: true,
    identity: { region: 'feishu', app_id: 'cli_app', tenant_key: 'tenant', bot_open_id: 'ou_bot' },
    admission: { type: 'all' },
  };
  expect(SourceSchema(source)).toEqual(source);
  expect(SourceRequestSchema({ ...source, conversation_history: true })).toEqual({
    ...source,
    conversation_history: true,
  });
  const fixtures = JSON.parse(
    readFileSync(
      new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
      'utf8',
    ),
  ) as Record<string, unknown[]>;
  const page = fixtures.ConversationHistoryPage![2];
  expect(ConversationHistoryPageSchema(page)).toEqual(page);
});

it.each([
  { type: 'telegram', bot_id: '77', admission: { type: 'all' } },
  {
    type: 'nyxid_relay',
    platform: 'telegram',
    issuer: 'https://fixture.example',
    key_id: 'fixture-key',
    route_ids: ['fixture-route'],
    admission: { type: 'all' },
  },
])('accepts optional and nullable member-join lists when writing $type sources', (source) => {
  expect(SourceRequestSchema(source)).toEqual(source);
  for (const ids of [null, [], ['-100']]) {
    const request = { ...source, member_join_chat_ids: ids };
    expect(SourceRequestSchema(request)).toEqual(request);
  }
  const response =
    source.type === 'nyxid_relay'
      ? { ...source, jwks_url: 'https://fixture.example/keys' }
      : source;
  expect(SourceSchema(response)).toEqual(response);
  expect(() => SourceSchema({ ...response, member_join_chat_ids: null })).toThrow();
});

it('keeps Telegram topic consent independent and discovers only close/reopen', () => {
  const source = { type: 'telegram', bot_id: '77', admission: { type: 'all' } };
  expect(SourceSchema(source)).toEqual(source);
  expect(SourceRequestSchema(source)).toEqual(source);
  const opted = { ...source, topic_lifecycle: true };
  expect(SourceSchema(opted)).toEqual(opted);
  expect(SourceRequestSchema(opted)).toEqual(opted);
  const caps = {
    manage_original_topic: true,
    actions: ['close', 'reopen'],
    expires_at: 1800000600,
  };
  expect(TopicLifecycleCapabilitiesSchema(caps)).toEqual(caps);
  expect(() => TopicLifecycleCapabilitiesSchema({ ...caps, actions: ['archive'] })).toThrow();
  expect(TopicLifecycleCapabilitiesSchema({ ...caps, list: 'original_thread' })).toEqual(caps);
  const request = { target: { type: 'event', event_ref: 'cmaeg1.ev.fixture' }, action: 'close' };
  expect(ManageThreadRequestSchema(request)).toEqual(request);
  const rename = { ...request, action: 'rename', name: 'Incident triage' };
  expect(ManageThreadRequestSchema(rename)).toEqual(rename);
  expect(() => TopicLifecycleCapabilitiesSchema({ ...caps, actions: ['rename'] })).toThrow();
});

it('retains independent Telegram creation consent and rejects a null archive duration', () => {
  const legacy = { type: 'telegram', bot_id: '77', admission: { type: 'all' } };
  expect(SourceSchema(legacy)).toEqual(legacy);
  expect(SourceSchema({ ...legacy, topic_creation: true })).toEqual({
    ...legacy,
    topic_creation: true,
  });
  const request = { name: 'Release planning' };
  expect(CreateThreadRequestSchema(request)).toEqual(request);
  expect(() => CreateThreadRequestSchema({ ...request, auto_archive_duration: null })).toThrow();
});

it('keeps Telegram member-join request nullability separate from stored sources', () => {
  const source = { type: 'telegram', bot_id: '77', admission: { type: 'all' } };
  for (const parse of [SourceRequestSchema, SourceSchema]) {
    expect(parse(source)).toEqual(source);
    for (const ids of [[], ['-100']]) {
      const value = { ...source, member_join_chat_ids: ids };
      expect(parse(value)).toEqual(value);
    }
    expect(() => parse({ ...source, member_join_chat_ids: [100] })).toThrow('member_join_chat_ids');
  }
  const nullUpdate = { ...source, member_join_chat_ids: null };
  expect(SourceRequestSchema(nullUpdate)).toEqual(nullUpdate);
  expect(() => SourceSchema(nullUpdate)).toThrow('member_join_chat_ids');
});
