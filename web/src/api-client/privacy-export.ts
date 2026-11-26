import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import type { OperationOptions } from './http.js';
import { query, read, segment } from './resources.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  text,
  type Output,
} from './schema.js';

const PATH = '/api/v1/account/privacy-export';
const inventory = object({
  owner_subject: text,
  data_class: enumeration('retained_conversation'),
  agents: array(object({ id: text, title: nullable(text) })),
  next: nullable(text),
});
const continuation = object({
  agent: text,
  turn: positiveInteger,
  after: integer,
  through: positiveInteger,
  limit: positiveInteger,
});
const transcript = object({
  owner_subject: text,
  manifest: object({
    format: enumeration('cma.retained-conversation-page'),
    version: positiveInteger,
    agent_id: text,
    through_seq: integer,
    window: object({
      turn: nullable(positiveInteger),
      after: nullable(integer),
      through: nullable(positiveInteger),
      limit: positiveInteger,
    }),
    coverage: object({
      raw_history: boolean,
      completeness: text,
      retention_window: text,
      gaps: text,
      includes: array(text),
      excludes: array(text),
    }),
    records: integer,
    size_bytes: integer,
    sha256: text,
    next: nullable(continuation),
  }),
  payload: text,
});

export type PrivacyExportInventory = Output<typeof inventory>;
type Continuation = Output<typeof continuation>;

export class PrivacyExports {
  constructor(private readonly config: ResolvedConfig) {}

  async inventory(owner: string, cursor?: string, options: OperationOptions = {}) {
    const page = await read(this.config, query(PATH, { cursor }), inventory, {
      ...options,
      cache: 'no-store',
      redirect: 'error',
    });
    const ids = page.agents.map(({ id }) => id);
    if (
      page.owner_subject !== owner ||
      ids.length > 20 ||
      ids.some((id, i) => !id || id <= (i === 0 ? (cursor ?? '') : ids[i - 1]!)) ||
      (page.next !== null && (ids.length !== 20 || page.next !== ids.at(-1)))
    ) {
      throw new ProtocolError('The export inventory did not match this account or page.');
    }
    return page;
  }

  async transcriptPage(
    owner: string,
    agent: string,
    cursor?: Continuation,
    options: OperationOptions = {},
  ): Promise<Output<typeof transcript>> {
    const path = `${PATH}/agents/${segment(agent)}/transcript`;
    const page = await read(this.config, query(path, cursor ?? { limit: 32 }), transcript, {
      ...options,
      cache: 'no-store',
      redirect: 'error',
    });
    if (
      page.owner_subject !== owner ||
      page.manifest.agent_id !== agent ||
      page.manifest.version !== 1
    ) {
      throw new ProtocolError('The export page did not match this account or Agent.');
    }
    const { window } = page.manifest;
    if (
      window.turn !== (cursor?.turn ?? null) ||
      window.after !== (cursor?.after ?? null) ||
      window.through !== (cursor?.through ?? null) ||
      window.limit !== (cursor?.limit ?? 32)
    ) {
      throw new ProtocolError('The export response did not match the requested page.');
    }
    await verifyPage(page.manifest, page.payload, new TextEncoder());
    options.signal?.throwIfAborted();
    return page;
  }

  async retainedConversation(owner: string, agent: string, signal: AbortSignal): Promise<Blob> {
    const parts = ['{"format":"cma.retained-conversation-export","version":1,"pages":['];
    const encoder = new TextEncoder();
    let bytes = parts[0]!.length + 2;
    let next: Continuation | null = null;
    let through: number | undefined;
    for (let count = 0; count < 256; count += 1) {
      const page = await this.transcriptPage(owner, agent, next ?? undefined, { signal });
      const { manifest, payload } = page;
      through ??= manifest.through_seq;
      if (
        manifest.through_seq !== through ||
        manifest.coverage.raw_history ||
        (manifest.next !== null &&
          (manifest.next.agent !== agent ||
            manifest.next.through !== through ||
            manifest.next.limit !== (next?.limit ?? 32) ||
            manifest.next.after <= (next?.after ?? 0) ||
            manifest.next.after < manifest.next.turn - 1 ||
            manifest.next.after >= through))
      ) {
        throw new ProtocolError('The export no longer matches the requested conversation.');
      }
      const record = (count === 0 ? '' : ',') + JSON.stringify({ manifest, payload });
      bytes += encoder.encode(record).byteLength;
      if (bytes > 16 * 1024 * 1024) break;
      parts.push(record);
      next = manifest.next;
      if (next === null) return new Blob([...parts, ']}'], { type: 'application/json' });
    }
    throw new ProtocolError('The conversation exceeds the export limit. No file was downloaded.');
  }
}

async function verifyPage(
  manifest: Output<typeof transcript>['manifest'],
  payload: string,
  encoder: TextEncoder,
) {
  const bytes = encoder.encode(payload);
  if (
    bytes.byteLength > 1024 * 1024 ||
    bytes.byteLength !== manifest.size_bytes ||
    (payload !== '' && !payload.endsWith('\n')) ||
    (payload.match(/\n/g)?.length ?? 0) !== manifest.records ||
    !/^[a-f0-9]{64}$/.test(manifest.sha256)
  ) {
    throw new ProtocolError('The export page did not pass its integrity check.');
  }
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  const hash = Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
  if (hash !== manifest.sha256) {
    throw new ProtocolError('The export page did not pass its SHA-256 check.');
  }
}
