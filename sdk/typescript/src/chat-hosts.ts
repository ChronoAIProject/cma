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

/** Typed phase vocabulary for one retained shared-host failure. */
export const CHAT_HOST_FAILURE_PHASES = [
  'runtime',
  'workspace_source',
  'workspace_setup',
  'credential',
  'cleanup',
  'runtime_lost',
] as const;

export type ChatHostFailurePhase = (typeof CHAT_HOST_FAILURE_PHASES)[number];

/** Bounded operator evidence for the host's most recent failure. */
export interface ChatHostFailure {
  readonly phase: ChatHostFailurePhase;
  readonly commandIndex?: number;
  readonly outputExcerpt?: string;
  readonly retryable: boolean;
  readonly cleanupPending: boolean;
  readonly occurredAt: number;
}

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
  readonly lastFailure?: ChatHostFailure;
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
    lastFailure: optionalFailure(host.last_failure, id),
    settingsRevision: nonNegativeInteger(
      host.settings_revision,
      `chat host ${id} settings revision`,
    ),
    createdAt: requiredInstant(host.created_at, `chat host ${id} creation time`),
  };
}

function optionalFailure(value: unknown, hostId: string): ChatHostFailure | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  if (typeof value !== 'object' || Array.isArray(value)) {
    throw new ProtocolError(`chat host ${hostId} failure was not an object`);
  }
  const failure = value as Record<string, unknown>;
  const phase = requiredText(failure.phase, `chat host ${hostId} failure phase`);
  if (!CHAT_HOST_FAILURE_PHASES.includes(phase as ChatHostFailurePhase)) {
    throw new ProtocolError(`chat host ${hostId} carried an unknown failure phase`);
  }
  const commandIndex = optionalNonNegativeInteger(
    failure.command_index,
    `chat host ${hostId} failure command index`,
  );
  const outputExcerpt = optionalOutput(failure.output_excerpt, hostId);
  if (
    (commandIndex !== undefined && phase !== 'workspace_setup') ||
    (outputExcerpt !== undefined && (phase !== 'workspace_setup' || commandIndex === undefined))
  ) {
    throw new ProtocolError(`chat host ${hostId} carried inconsistent setup failure evidence`);
  }
  return {
    phase: phase as ChatHostFailurePhase,
    commandIndex,
    outputExcerpt,
    retryable: requiredBoolean(failure.retryable, `chat host ${hostId} failure retryability`),
    cleanupPending: requiredBoolean(
      failure.cleanup_pending,
      `chat host ${hostId} failure cleanup state`,
    ),
    occurredAt: requiredInstant(failure.occurred_at, `chat host ${hostId} failure time`),
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

function optionalNonNegativeInteger(value: unknown, field: string): number | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  return nonNegativeInteger(value, field);
}

function requiredBoolean(value: unknown, field: string): boolean {
  if (typeof value !== 'boolean') {
    throw new ProtocolError(`${field} was not boolean`);
  }
  return value;
}

function optionalOutput(value: unknown, hostId: string): string | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  if (typeof value !== 'string' || new TextEncoder().encode(value).byteLength > 32 * 1024) {
    throw new ProtocolError(`chat host ${hostId} failure output exceeded its byte bound`);
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
