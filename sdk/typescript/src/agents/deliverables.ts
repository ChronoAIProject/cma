import type { ResolvedConfig } from '../config.js';
import type { DocumentAccessOptions } from '../documents.js';
import { ProtocolError } from '../errors.js';
import {
  readBytes,
  requestResponse,
  throwIfSignalAborted,
  withDeadline,
  type RequestOptions,
} from '../http.js';
import { input, query, read, segment } from '../resources.js';
import {
  array,
  enumeration,
  integer,
  object,
  positiveInteger,
  tagged,
  text,
  type Output,
} from '../schema.js';

const resource = tagged(
  {
    agent_artifact: object({ kind: enumeration('agent_artifact'), id: text }),
    document: object({ kind: enumeration('document'), id: text, revision: positiveInteger }),
    live_file: object({ kind: enumeration('live_file'), path: text }),
  },
  'kind',
);
const reference = object({
  agent_id: text,
  resource,
  name: text,
  media_type: enumeration(
    'text/plain',
    'text/markdown',
    'application/json',
    'application/pdf',
    'application/octet-stream',
    'image/png',
    'image/jpeg',
    'image/webp',
  ),
  size_bytes: integer,
  sha256: text,
});
const delivery = object({
  id: text,
  reference,
  status: enumeration(
    'ready',
    'unchecked',
    'changed',
    'version_changed',
    'missing',
    'denied',
    'not_ready',
    'too_large',
    'unavailable',
  ),
});
const collection = object({
  agent_id: text,
  answer_id: text,
  turn_sequence: positiveInteger,
  items: array(delivery),
});
const reading = object({
  agent_id: text,
  answer_id: text,
  turn_sequence: positiveInteger,
  item: delivery,
});
const description = object({ reference, marker: text });

export type DeliverableResource = Output<typeof resource>;
export type DeliverableReference = Output<typeof reference>;
export type AnswerDeliverable = Output<typeof delivery>;
export type AnswerDeliverables = Output<typeof collection>;
export type AnswerDeliverableReading = Output<typeof reading>;
export type DeliverableDescription = Output<typeof description>;
export interface DeliverableReadOptions extends DocumentAccessOptions {
  readonly verify?: boolean;
}

function answerSequence(value: unknown): number {
  const answer = text(value);
  if (!/^event:[1-9]\d*$/.test(answer)) throw new ProtocolError('invalid final answer identity');
  return positiveInteger(Number(answer.slice(6)));
}

function deliveryId(value: unknown): string {
  const id = text(value);
  if (!/^delivery_[a-f0-9]{64}$/.test(id)) throw new ProtocolError('invalid delivery identity');
  return id;
}

function selectedResource(value: unknown): DeliverableResource {
  const selected = resource(value);
  if (selected.kind === 'live_file') {
    if (
      new TextEncoder().encode(selected.path).length > 1024 ||
      /[\p{Cc}\\]/u.test(selected.path) ||
      selected.path.split('/').some((part) => part === '' || part === '.' || part === '..')
    ) {
      throw new ProtocolError('invalid home-relative file selector');
    }
  } else if (
    (selected.kind === 'agent_artifact' && !/^art_[a-f0-9]{64}$/.test(selected.id)) ||
    (selected.kind === 'document' && !/^doc_[a-f0-9]{32}$/.test(selected.id))
  ) {
    throw new ProtocolError('invalid immutable resource selector');
  }
  return selected;
}

function validateItem(value: AnswerDeliverable, agentId: string, id?: string): AnswerDeliverable {
  deliveryId(value.id);
  if (id !== undefined && value.id !== id)
    throw new ProtocolError('delivery identity does not match the request');
  validateReference(value.reference, agentId);
  return value;
}

function validateReference(item: DeliverableReference, agentId: string): DeliverableReference {
  if (
    item.agent_id !== agentId ||
    item.name.trim() !== item.name ||
    item.name === '' ||
    item.name === '.' ||
    item.name === '..' ||
    /[\p{Cc}/\\]/u.test(item.name) ||
    new TextEncoder().encode(item.name).length > 160 ||
    !/^[a-f0-9]{64}$/.test(item.sha256)
  ) {
    throw new ProtocolError('delivery metadata does not match the selected resource');
  }
  const selected = item.resource;
  if (selected.kind === 'live_file') {
    if (
      item.media_type !== 'application/octet-stream' ||
      item.size_bytes > 32 * 1024 * 1024 ||
      new TextEncoder().encode(selected.path).length > 1024 ||
      /[\p{Cc}\\]/u.test(selected.path) ||
      selected.path.split('/').some((part) => part === '' || part === '.' || part === '..')
    ) {
      throw new ProtocolError('invalid live file reference');
    }
  } else if (
    item.size_bytes === 0 ||
    item.size_bytes > 1024 * 1024 ||
    (selected.kind === 'agent_artifact' && !/^art_[a-f0-9]{64}$/.test(selected.id)) ||
    (selected.kind === 'document' &&
      (!/^doc_[a-f0-9]{32}$/.test(selected.id) || item.media_type !== 'text/markdown'))
  ) {
    throw new ProtocolError('invalid immutable resource reference');
  }
  return item;
}

function validateIdentity(
  value: { agent_id: string; answer_id: string; turn_sequence: number },
  agentId: string,
  answerId: string,
) {
  if (
    value.agent_id !== agentId ||
    value.answer_id !== answerId ||
    value.turn_sequence >= answerSequence(answerId)
  ) {
    throw new ProtocolError('delivery response does not belong to this answer');
  }
}

function path(agentId: string, answerId: string): string {
  input((value) => {
    answerSequence(value);
    return text(value);
  }, answerId);
  return `/api/v1/agents/${segment(agentId)}/answers/${segment(answerId)}/deliverables`;
}

function access(options: DocumentAccessOptions): RequestOptions {
  if (options.shareToken !== undefined)
    input((value) => {
      if (!/^[a-f0-9]{64}$/.test(text(value)))
        throw new ProtocolError('invalid document share token');
      return value;
    }, options.shareToken);
  return {
    ...options,
    headers:
      options.shareToken === undefined ? {} : { 'X-CMA-Document-Share-Token': options.shareToken },
  };
}

export function describeDeliverable(
  config: ResolvedConfig,
  agentId: string,
  selected: DeliverableResource,
  options: DocumentAccessOptions = {},
) {
  const requested = input(selectedResource, selected);
  return read(
    config,
    `/api/v1/agents/${segment(agentId)}/deliverable-references`,
    (value) => {
      const result = description(value);
      validateReference(result.reference, agentId);
      if (JSON.stringify(result.reference.resource) !== JSON.stringify(requested)) {
        throw new ProtocolError('verified descriptor does not match the selected resource');
      }
      const encoded = /^<!--cma-deliverable:v1:([A-Za-z0-9_-]{1,4096})-->$/.exec(
        result.marker,
      )?.[1];
      if (encoded === undefined) throw new ProtocolError('invalid deliverable marker');
      try {
        const bytes = Uint8Array.from(
          atob(encoded.replaceAll('-', '+').replaceAll('_', '/')),
          (character) => character.charCodeAt(0),
        );
        const canonical = btoa(String.fromCharCode(...bytes))
          .replaceAll('+', '-')
          .replaceAll('/', '_')
          .replace(/=+$/, '');
        if (
          canonical !== encoded ||
          new TextDecoder('utf-8', { fatal: true }).decode(bytes) !==
            JSON.stringify(result.reference)
        ) {
          throw new Error('descriptor mismatch');
        }
      } catch (cause) {
        throw new ProtocolError('deliverable marker does not match its verified metadata', {
          cause,
        });
      }
      return result;
    },
    { ...access(options), method: 'POST', body: requested, expectedStatus: 200 },
  );
}

export function listAnswerDeliverables(
  config: ResolvedConfig,
  agentId: string,
  answerId: string,
  options: DocumentAccessOptions = {},
) {
  return read(
    config,
    path(agentId, answerId),
    (value) => {
      const result = collection(value);
      validateIdentity(result, agentId, answerId);
      if (
        result.items.length > 32 ||
        new Set(result.items.map((item) => item.id)).size !== result.items.length
      ) {
        throw new ProtocolError('answer deliveries exceed their distinct reference bound');
      }
      for (const item of result.items) validateItem(item, agentId);
      return result;
    },
    access(options),
  );
}

export function readAnswerDeliverable(
  config: ResolvedConfig,
  agentId: string,
  answerId: string,
  id: string,
  options: DeliverableReadOptions = {},
) {
  input(deliveryId, id);
  return read(
    config,
    query(`${path(agentId, answerId)}/${segment(id)}`, {
      verify: options.verify === undefined ? undefined : String(options.verify),
    }),
    (value) => {
      const result = reading(value);
      validateIdentity(result, agentId, answerId);
      validateItem(result.item, agentId, id);
      return result;
    },
    access(options),
  );
}

function requireDownload(current: AnswerDeliverable, expected: AnswerDeliverable) {
  if (
    current.id !== expected.id ||
    JSON.stringify(current.reference) !== JSON.stringify(expected.reference) ||
    !['ready', 'unchecked', 'version_changed'].includes(current.status)
  ) {
    throw new ProtocolError(`delivery is unavailable or changed (${current.status})`);
  }
}

export function downloadAnswerDeliverable(
  config: ResolvedConfig,
  agentId: string,
  answerId: string,
  expected: AnswerDeliverable,
  options: DocumentAccessOptions = {},
) {
  const selected = input((value) => validateItem(delivery(value), agentId), expected);
  const target = `${path(agentId, answerId)}/${segment(selected.id)}/download`;
  const request = access(options);
  return withDeadline(config, options, async (signal, requestId) => {
    const scoped = { ...options, signal, requestId };
    requireDownload(
      (await readAnswerDeliverable(config, agentId, answerId, selected.id, scoped)).item,
      selected,
    );
    const response = await requestResponse(config, target, {
      ...request,
      signal,
      requestId,
      expectedStatus: 200,
    });
    if (
      response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
      'application/octet-stream'
    ) {
      await response.body?.cancel();
      throw new ProtocolError('deliverable download did not return opaque bytes');
    }
    const bytes = await readBytes(response, signal, Math.max(1, selected.reference.size_bytes));
    const digest = new Uint8Array(
      await crypto.subtle.digest('SHA-256', Uint8Array.from(bytes).buffer),
    );
    const sha256 = Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
    if (bytes.length !== selected.reference.size_bytes || sha256 !== selected.reference.sha256) {
      throw new ProtocolError('deliverable bytes do not match this answer version');
    }
    throwIfSignalAborted(signal);
    requireDownload(
      (await readAnswerDeliverable(config, agentId, answerId, selected.id, scoped)).item,
      selected,
    );
    throwIfSignalAborted(signal);
    return bytes;
  });
}
