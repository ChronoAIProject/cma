import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';
import {
  ActivityRoutingSchema,
  ActivitySchema,
  AttachmentSchema,
  AttachmentSourceSchema,
  ChannelResponseSchema,
  ConsumerSummarySchema,
  ContextResponseSchema,
  EventContextSnapshotSchema,
  PublicActivitySchema,
  RouteResolutionSchema,
  CustomAttachmentsSchema,
  DefaultReplySchema,
  RoutingPolicySchema,
  SourceRequestSchema,
  SourceSchema,
} from '../src/cmaeg/types.js';

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
  readFileSync(new URL('../../../api/crates/cmaeg/openapi/v1.json', import.meta.url), 'utf8'),
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
    const source = fileURLToPath(new URL('../src/cmaeg/types.js', import.meta.url));
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
      new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
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

describe('CMAEG custom source attachments', () => {
  const examples = JSON.parse(
    readFileSync(
      new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
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
      new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
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
      new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
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
      new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
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
