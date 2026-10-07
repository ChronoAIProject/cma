import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';
import {
  AttachmentSchema,
  AttachmentSourceSchema,
  CustomAttachmentsSchema,
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
