/** Administrative inventory and retirement for deployment-managed chat hosts. */

import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import {
  pageTarget,
  request,
  requestPage,
  requestPages,
  type OperationOptions,
  type Page,
  type PageRequest,
  type PageWalkOptions,
} from './http.js';

const CHAT_HOST_COLLECTION = '/api/v1/admin/chat/hosts';

/** Lifecycle values emitted by the chat-host administration contract. */
export const CHAT_HOST_STATUSES = [
  'provisioning',
  'active',
  'pausing',
  'paused',
  'reviving',
  'retired',
] as const;

export type ChatHostStatus = (typeof CHAT_HOST_STATUSES)[number];

/** Redacted operator projection of one shared chat runtime. */
export interface ChatHost {
  readonly id: string;
  readonly owner: string;
  readonly status: ChatHostStatus;
  readonly threadCount: number;
  readonly runtimeVersion?: string;
  readonly pausedAt?: number;
  readonly revivedAt?: number;
  readonly checkpointCapturedAt?: number;
  readonly settingsRevision: number;
  readonly createdAt: number;
}

export type ChatHostPageRequest = PageRequest;
export type ChatHostListRequest = PageWalkOptions;

/** Decode one chat-host projection without accepting lifecycle-internal data. */
export function toChatHost(raw: unknown): ChatHost {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError('chat host payload was not an object');
  }
  const host = raw as Record<string, unknown>;
  const id = requiredText(host.id, 'chat host id');
  const status = requiredText(host.status, `chat host ${id} status`);
  if (!CHAT_HOST_STATUSES.includes(status as ChatHostStatus)) {
    throw new ProtocolError(`chat host ${id} carried an unknown lifecycle`);
  }
  return {
    id,
    owner: requiredText(host.owner, `chat host ${id} owner`),
    status: status as ChatHostStatus,
    threadCount: nonNegativeInteger(host.thread_count, `chat host ${id} thread count`),
    runtimeVersion: optionalText(host.runtime_version, `chat host ${id} runtime version`),
    pausedAt: optionalInstant(host.paused_at, `chat host ${id} pause time`),
    revivedAt: optionalInstant(host.revived_at, `chat host ${id} revival time`),
    checkpointCapturedAt: optionalInstant(
      host.checkpoint_captured_at,
      `chat host ${id} checkpoint time`,
    ),
    settingsRevision: nonNegativeInteger(
      host.settings_revision,
      `chat host ${id} settings revision`,
    ),
    createdAt: requiredInstant(host.created_at, `chat host ${id} creation time`),
  };
}

function decodePage(body: unknown): readonly ChatHost[] {
  if (!Array.isArray(body)) {
    throw new ProtocolError('chat host list was not a list');
  }
  return body.map(toChatHost);
}

/** Read one bounded newest-first host page. */
export async function listChatHostsPage(
  config: ResolvedConfig,
  page: ChatHostPageRequest = {},
  options: OperationOptions = {},
): Promise<Page<ChatHost>> {
  const target = pageTarget(CHAT_HOST_COLLECTION, page);
  return requestPage(config, CHAT_HOST_COLLECTION, target, decodePage, options);
}

/** Walk every host page within the caller-selected resource ceilings. */
export async function listChatHosts(
  config: ResolvedConfig,
  options: ChatHostListRequest = {},
): Promise<readonly ChatHost[]> {
  return requestPages(config, CHAT_HOST_COLLECTION, decodePage, options);
}

/** Retire one active or paused host and return its resulting projection. */
export async function retireChatHost(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
): Promise<ChatHost> {
  if (id.trim() === '') {
    throw new ValidationError('chat host id must not be empty');
  }
  const payload = await request(
    config,
    `${CHAT_HOST_COLLECTION}/${encodeURIComponent(id)}/retire`,
    { ...options, method: 'POST', expectedStatus: 200 },
  );
  return toChatHost(payload);
}

function requiredText(value: unknown, field: string): string {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new ProtocolError(`${field} was not non-empty text`);
  }
  return value;
}

function optionalText(value: unknown, field: string): string | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  return requiredText(value, field);
}

function nonNegativeInteger(value: unknown, field: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new ProtocolError(`${field} was not a non-negative safe integer`);
  }
  return value;
}

function requiredInstant(value: unknown, field: string): number {
  if (typeof value !== 'string') {
    throw new ProtocolError(`${field} was not an RFC 3339 timestamp`);
  }
  const parsed = Date.parse(value);
  if (Number.isNaN(parsed)) {
    throw new ProtocolError(`${field} was not an RFC 3339 timestamp`);
  }
  return parsed;
}

function optionalInstant(value: unknown, field: string): number | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  return requiredInstant(value, field);
}
