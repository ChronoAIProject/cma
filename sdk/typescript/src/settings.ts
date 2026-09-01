/** Deployment-owned settings used to create and operate chat hosts. */

import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, type OperationOptions } from './http.js';
import type { ReasoningEffort } from './types.js';

const BASE = '/api/v1/admin/settings';
const EFFORTS: readonly ReasoningEffort[] = ['minimal', 'low', 'medium', 'high', 'x_high'];
const UTF8 = new TextEncoder();

function byteLength(value: string): number {
  return UTF8.encode(value).byteLength;
}

export interface ChatWorkspaceSource {
  readonly repositoryUrl: string;
  readonly branch: string;
}

export interface ChatAgentSettings {
  readonly model: string | null;
  readonly reasoningEffort: ReasoningEffort;
  readonly instructions: string;
  readonly recordToolBodies: boolean;
}

export interface ChatInactivity {
  readonly pauseAfterSeconds: number;
}

export interface ChatSetup {
  readonly commands: readonly string[];
}

export interface ChatHostSize {
  readonly cpuMillis: number;
  readonly memoryMib: number;
}

export interface ChatHostSettings {
  readonly size: ChatHostSize | null;
  readonly maxThreads: number;
}

/** Complete replacement value for the optional platform chat setting. */
export interface ChatSettings {
  readonly enabled: boolean;
  readonly workspace: ChatWorkspaceSource;
  readonly agent: ChatAgentSettings;
  readonly messagePrefixes: readonly string[];
  readonly inactivity: ChatInactivity;
  readonly setup: ChatSetup;
  readonly host: ChatHostSettings;
}

/** Chat portion of a revisioned platform settings response. */
export interface StoredChatSettings {
  readonly revision: number;
  readonly chat: ChatSettings | null;
  readonly previousChat?: ChatSettings | null;
}

/** Decode the complete chat settings object returned by the control plane. */
export function toChatSettings(raw: unknown): ChatSettings {
  const value = object(raw, 'chat settings');
  const workspace = object(value.workspace, 'chat workspace');
  const agent = object(value.agent, 'chat agent settings');
  const inactivity = object(value.inactivity, 'chat inactivity');
  const setup = object(value.setup, 'chat setup');
  const host = object(value.host, 'chat host settings');
  const model = agent.model;
  if (model !== null && typeof model !== 'string') {
    throw new ProtocolError('chat model was neither text nor null');
  }
  const reasoningEffort = requiredString(agent.reasoning_effort, 'chat reasoning effort');
  if (!EFFORTS.includes(reasoningEffort as ReasoningEffort)) {
    throw new ProtocolError('chat reasoning effort was not recognized');
  }
  return validateChatSettings(
    {
      enabled: requiredBoolean(value.enabled, 'chat enabled flag'),
      workspace: {
        repositoryUrl: requiredString(workspace.repository_url, 'chat repository URL'),
        branch: requiredString(workspace.branch, 'chat branch'),
      },
      agent: {
        model,
        reasoningEffort: reasoningEffort as ReasoningEffort,
        instructions: requiredString(agent.instructions, 'chat instructions'),
        recordToolBodies: requiredBoolean(agent.record_tool_bodies, 'chat tool-body flag'),
      },
      messagePrefixes: stringArray(value.message_prefixes, 'chat message prefixes'),
      inactivity: {
        pauseAfterSeconds: integer(inactivity.pause_after_seconds, 'chat pause interval'),
      },
      setup: { commands: stringArray(setup.commands, 'chat setup commands') },
      host: {
        size: host.size === null ? null : toHostSize(host.size),
        maxThreads: integer(host.max_threads, 'chat thread cap'),
      },
    },
    ProtocolError,
  );
}

/** Decode the chat portion of a revisioned platform settings response. */
export function toStoredChatSettings(raw: unknown): StoredChatSettings {
  const stored = object(raw, 'platform settings response');
  const revision = integer(stored.revision, 'platform settings revision');
  const settings = object(stored.settings, 'platform settings');
  const previous =
    stored.previous === undefined
      ? undefined
      : object(stored.previous, 'previous platform settings');
  return {
    revision,
    chat: nullableChat(settings.chat),
    previousChat: previous === undefined ? undefined : nullableChat(previous.chat),
  };
}

/** Read the deployment's current chat setting and conditional-write revision. */
export async function readChatSettings(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<StoredChatSettings> {
  return toStoredChatSettings(await request(config, BASE, { ...options, expectedStatus: 200 }));
}

/** Replace only the chat setting under the platform settings revision CAS. */
export async function changeChatSettings(
  config: ResolvedConfig,
  expectedRevision: number,
  chat: ChatSettings | null,
  options: OperationOptions = {},
): Promise<StoredChatSettings> {
  requireNonNegativeInteger(expectedRevision, 'platform settings revision', ValidationError);
  return toStoredChatSettings(
    await request(config, BASE, {
      ...options,
      method: 'PATCH',
      expectedStatus: 200,
      body: {
        expected_revision: expectedRevision,
        chat: chat === null ? null : chatRequest(validateChatSettings(chat, ValidationError)),
      },
    }),
  );
}

function chatRequest(chat: ChatSettings): Record<string, unknown> {
  return {
    enabled: chat.enabled,
    workspace: {
      repository_url: chat.workspace.repositoryUrl,
      branch: chat.workspace.branch,
    },
    agent: {
      model: chat.agent.model,
      reasoning_effort: chat.agent.reasoningEffort,
      instructions: chat.agent.instructions,
      record_tool_bodies: chat.agent.recordToolBodies,
    },
    message_prefixes: [...chat.messagePrefixes],
    inactivity: { pause_after_seconds: chat.inactivity.pauseAfterSeconds },
    setup: { commands: [...chat.setup.commands] },
    host: {
      size:
        chat.host.size === null
          ? null
          : {
              cpu_millis: chat.host.size.cpuMillis,
              memory_mib: chat.host.size.memoryMib,
            },
      max_threads: chat.host.maxThreads,
    },
  };
}

type ErrorConstructor = new (message: string) => Error;

function validateChatSettings<T extends ChatSettings>(chat: T, Failure: ErrorConstructor): T {
  if (
    byteLength(chat.workspace.repositoryUrl) === 0 ||
    byteLength(chat.workspace.repositoryUrl) > 512
  ) {
    throw new Failure('chat repository URL must contain 1 through 512 bytes');
  }
  let repository: URL;
  try {
    repository = new URL(chat.workspace.repositoryUrl);
  } catch {
    throw new Failure('chat repository URL must be an HTTPS URL');
  }
  if (
    repository.protocol !== 'https:' ||
    repository.username !== '' ||
    repository.password !== '' ||
    repository.search !== '' ||
    repository.hash !== ''
  ) {
    throw new Failure('chat repository URL must be credential-free HTTPS');
  }
  const basename = repository.pathname
    .split('/')
    .filter(Boolean)
    .at(-1)
    ?.replace(/\.git$/, '');
  if (basename === undefined || !/^(?!\.{1,2}$)[A-Za-z0-9._-]{1,255}$/.test(basename)) {
    throw new Failure('chat repository URL must end in a safe repository name');
  }
  if (
    byteLength(chat.workspace.branch) === 0 ||
    byteLength(chat.workspace.branch) > 200 ||
    /\s/.test(chat.workspace.branch)
  ) {
    throw new Failure('chat branch must contain 1 through 200 non-whitespace characters');
  }
  if (!EFFORTS.includes(chat.agent.reasoningEffort)) {
    throw new Failure('chat reasoning effort was not recognized');
  }
  if (chat.messagePrefixes.length > 8) {
    throw new Failure('chat message prefixes must contain at most 8 entries');
  }
  for (const prefix of chat.messagePrefixes) {
    if (prefix.trim() === '' || byteLength(prefix) > 256 || /[\r\n]/.test(prefix)) {
      throw new Failure(
        'chat message prefixes must be non-empty single lines of at most 256 characters',
      );
    }
  }
  requireRange(chat.inactivity.pauseAfterSeconds, 60, 86_400, 'chat pause interval', Failure);
  if (
    chat.setup.commands.length > 16 ||
    chat.setup.commands.some((command) => byteLength(command) > 4_096)
  ) {
    throw new Failure('chat setup must contain at most 16 commands of at most 4096 characters');
  }
  requireRange(chat.host.maxThreads, 1, 50, 'chat thread cap', Failure);
  if (chat.host.size !== null) {
    requireRange(chat.host.size.cpuMillis, 1, 4_294_967_295, 'chat host CPU', Failure);
    requireRange(chat.host.size.memoryMib, 1, 4_294_967_295, 'chat host memory', Failure);
  }
  return chat;
}

function nullableChat(raw: unknown): ChatSettings | null {
  return raw === undefined || raw === null ? null : toChatSettings(raw);
}

function toHostSize(raw: unknown): ChatHostSize {
  const value = object(raw, 'chat host size');
  return {
    cpuMillis: integer(value.cpu_millis, 'chat host CPU'),
    memoryMib: integer(value.memory_mib, 'chat host memory'),
  };
}

function object(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function requiredString(raw: unknown, name: string): string {
  if (typeof raw !== 'string') {
    throw new ProtocolError(`${name} was not text`);
  }
  return raw;
}

function requiredBoolean(raw: unknown, name: string): boolean {
  if (typeof raw !== 'boolean') {
    throw new ProtocolError(`${name} was not boolean`);
  }
  return raw;
}

function stringArray(raw: unknown, name: string): readonly string[] {
  if (!Array.isArray(raw) || raw.some((entry) => typeof entry !== 'string')) {
    throw new ProtocolError(`${name} was not a text array`);
  }
  return raw as string[];
}

function integer(raw: unknown, name: string): number {
  requireNonNegativeInteger(raw, name, ProtocolError);
  return raw as number;
}

function requireNonNegativeInteger(
  raw: unknown,
  name: string,
  Failure: ErrorConstructor,
): asserts raw is number {
  if (typeof raw !== 'number' || !Number.isSafeInteger(raw) || raw < 0) {
    throw new Failure(`${name} must be a non-negative safe integer`);
  }
}

function requireRange(
  raw: unknown,
  minimum: number,
  maximum: number,
  name: string,
  Failure: ErrorConstructor,
): asserts raw is number {
  requireNonNegativeInteger(raw, name, Failure);
  if (raw < minimum || raw > maximum) {
    throw new Failure(`${name} must be from ${String(minimum)} through ${String(maximum)}`);
  }
}
