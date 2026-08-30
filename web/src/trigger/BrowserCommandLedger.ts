import { ApiError, TransportError } from '../api/client';
import { newIntentKey } from './idempotency';

export const COMMAND_RETENTION_MS = 24 * 60 * 60 * 1_000;
export const COMMAND_LEASE_MS = 60 * 1_000;
export const COMMAND_STORAGE_PREFIX = 'cma:trigger-command-ledger:v1:';
export const COMMAND_LEDGER_EVENT = 'cma:trigger-command-ledger-change';
const VERSION = 1;
const COMMAND_DATABASE_NAME = 'cma-trigger-command-ledger';
const COMMAND_DATABASE_VERSION = 1;
const COMMAND_DATABASE_STORE = 'owner-envelopes';
const COMMAND_BROADCAST_CHANNEL = 'cma:trigger-command-ledger:v1';

export interface BrowserCommandIntent<TRequest = unknown> {
  readonly slot: string;
  readonly resource: string;
  readonly action: string;
  /** The exact request fields used to produce the public SDK call. */
  readonly request: TRequest;
  /** Reuse an exact completed command key when this resource action is one-shot. */
  readonly replayCompleted?: boolean;
  /**
   * Store the request only when it contains no credential or private prompt.
   * Otherwise only its SHA-256 fingerprint is durable.
   */
  readonly persistRequest?: boolean;
  /** Safe reconstruction data which is not itself part of the HTTP request. */
  readonly retained?: unknown;
}

export interface PendingBrowserCommand {
  readonly slot: string;
  readonly label: string;
  readonly resource: string;
  readonly action: string;
  readonly request?: unknown;
  readonly retained?: unknown;
  /** True after reload when an intentionally unpersisted body must be re-entered exactly. */
  readonly requiresRequestReconstruction: boolean;
  readonly expiresAt: number;
}

interface CommandIdentity {
  readonly slot: string;
  readonly resource: string;
  readonly action: string;
}

function sameIdentity(
  pending: PendingBrowserCommand | null,
  identity: CommandIdentity,
): pending is PendingBrowserCommand {
  return (
    pending !== null &&
    pending.slot === identity.slot &&
    pending.resource === identity.resource &&
    pending.action === identity.action
  );
}

/** Use a persisted non-secret request only for its exact resource/action slot. */
export function retainedCommandRequest<TRequest>(
  pending: PendingBrowserCommand | null,
  identity: CommandIdentity,
  fallback: TRequest,
  parse: (value: unknown) => TRequest | null,
): TRequest {
  if (!sameIdentity(pending, identity) || pending.request === undefined) return fallback;
  const parsed = parse(pending.request);
  if (parsed === null) {
    throw new CommandLedgerUnavailableError(
      'The retained command request does not match the current resource.',
    );
  }
  return parsed;
}

/** Read non-secret reconstruction metadata only for its exact command. */
export function retainedCommandMetadata<T>(
  pending: PendingBrowserCommand | null,
  identity: CommandIdentity,
  parse: (value: unknown) => T | null,
): T | undefined {
  if (!sameIdentity(pending, identity) || pending.retained === undefined) return undefined;
  const parsed = parse(pending.retained);
  if (parsed === null) {
    throw new CommandLedgerUnavailableError(
      'The retained command metadata does not match the current resource.',
    );
  }
  return parsed;
}

interface StoredCommand {
  readonly version: 1;
  readonly slot: string;
  readonly label: string;
  readonly resource: string;
  readonly action: string;
  readonly key: string;
  readonly fingerprint: string;
  readonly request?: unknown;
  readonly retained?: unknown;
  readonly createdAt: number;
  readonly expiresAt: number;
  readonly completedAt?: number;
  readonly leaseHolder: string | null;
  readonly leaseToken: string | null;
  readonly leaseExpiresAt: number | null;
}

interface StoredEnvelope {
  readonly version: 1;
  readonly commands: readonly StoredCommand[];
}

interface PreparedIntent<TRequest> {
  readonly intent: BrowserCommandIntent<TRequest>;
  readonly fingerprint: string;
  readonly storedRequest?: unknown;
  readonly storedRetained?: unknown;
}

interface RuntimeCommand {
  readonly fingerprint: string;
  readonly request: unknown;
  readonly send: (key: string, request: unknown) => Promise<unknown>;
  inFlight: Promise<unknown> | null;
}

export interface CommandCoordinator {
  readonly exclusive: <T>(name: string, task: () => Promise<T>) => Promise<T>;
}

interface BrowserCommandLedgerOptions {
  readonly ownerId: string;
  readonly storage?: Storage;
  readonly coordinator?: CommandCoordinator;
  readonly now?: () => number;
  readonly instanceId?: string;
}

interface CommandStorage {
  readonly getItem: (key: string) => Promise<string | null>;
  readonly setItem: (key: string, value: string) => Promise<void>;
  readonly removeItem: (key: string) => Promise<void>;
}

export class CommandDescriptorMismatchError extends Error {
  constructor() {
    super(
      'This action does not match the retained command. CMA will not send a changed body with its idempotency key.',
    );
    this.name = 'CommandDescriptorMismatchError';
  }
}

export class CommandRequestUnavailableError extends Error {
  constructor() {
    super(
      'The retained command contains private text that CMA did not store in this browser. Reconcile it from the original tab or wait for the retained command to expire.',
    );
    this.name = 'CommandRequestUnavailableError';
  }
}

export class CommandLedgerUnavailableError extends Error {
  constructor(message = 'This browser cannot safely retain and coordinate this command.') {
    super(message);
    this.name = 'CommandLedgerUnavailableError';
  }
}

class CommandResourceBusyError extends Error {
  constructor(label: string) {
    super(`${label} must be reconciled before starting another action on this Launch.`);
    this.name = 'CommandResourceBusyError';
  }
}

class CommandExecutionClaimedError extends TransportError {
  constructor() {
    super('This exact command is still being submitted by another CMA tab.');
    this.name = 'CommandExecutionClaimedError';
  }
}

/** True only when another same-origin CMA tab owns the live command lease. */
export function commandExecutionIsClaimed(cause: unknown): boolean {
  return cause instanceof CommandExecutionClaimedError;
}

export function commandOutcomeIsUnresolved(cause: unknown): boolean {
  return (
    cause instanceof TransportError ||
    (cause instanceof ApiError && cause.status === 409 && cause.code === 'idempotency_in_flight')
  );
}

function checkedString(value: unknown, field: string, maximum = 512): string {
  const containsControlCharacter =
    typeof value === 'string' &&
    Array.from(value).some((character) => {
      const codePoint = character.codePointAt(0);
      return codePoint !== undefined && (codePoint <= 31 || codePoint === 127);
    });
  if (
    typeof value !== 'string' ||
    value.length === 0 ||
    value.length > maximum ||
    containsControlCharacter
  ) {
    throw new CommandLedgerUnavailableError(`The retained command has an invalid ${field}.`);
  }
  return value;
}

function checkedTime(value: unknown, field: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new CommandLedgerUnavailableError(`The retained command has an invalid ${field}.`);
  }
  return value;
}

function canonicalJson(value: unknown, seen = new Set<object>()): string {
  if (value === null) return 'null';
  switch (typeof value) {
    case 'boolean':
    case 'string':
      return JSON.stringify(value);
    case 'number':
      if (!Number.isFinite(value)) {
        throw new CommandLedgerUnavailableError('A command request contains a non-finite number.');
      }
      return JSON.stringify(value);
    case 'object': {
      if (seen.has(value)) {
        throw new CommandLedgerUnavailableError('A command request contains a cycle.');
      }
      seen.add(value);
      try {
        if (Array.isArray(value)) {
          return `[${value.map((item) => canonicalJson(item, seen)).join(',')}]`;
        }
        const prototype = Object.getPrototypeOf(value);
        if (prototype !== Object.prototype && prototype !== null) {
          throw new CommandLedgerUnavailableError(
            'A command request contains a value that cannot be retained safely.',
          );
        }
        const record = value as Record<string, unknown>;
        return `{${Object.keys(record)
          .sort()
          .map((key) => {
            const item = record[key];
            if (item === undefined) {
              throw new CommandLedgerUnavailableError(
                'A command request contains an undefined field.',
              );
            }
            return `${JSON.stringify(key)}:${canonicalJson(item, seen)}`;
          })
          .join(',')}}`;
      } finally {
        seen.delete(value);
      }
    }
    default:
      throw new CommandLedgerUnavailableError(
        'A command request contains a value that cannot be serialized.',
      );
  }
}

async function sha256(value: string): Promise<string> {
  if (typeof crypto === 'undefined' || crypto.subtle === undefined) {
    throw new CommandLedgerUnavailableError(
      'This browser cannot fingerprint commands before sending them.',
    );
  }
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value));
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

function browserStorage(): Storage {
  try {
    if (typeof window === 'undefined') throw new Error('window is unavailable');
    return window.localStorage;
  } catch (cause) {
    throw new CommandLedgerUnavailableError(
      cause instanceof Error
        ? `This browser cannot retain commands: ${cause.message}`
        : 'This browser cannot retain commands.',
    );
  }
}

class SynchronousCommandStorage implements CommandStorage {
  readonly #storage: Storage;

  constructor(storage: Storage) {
    this.#storage = storage;
  }

  async getItem(key: string): Promise<string | null> {
    return this.#storage.getItem(key);
  }

  async setItem(key: string, value: string): Promise<void> {
    this.#storage.setItem(key, value);
  }

  async removeItem(key: string): Promise<void> {
    this.#storage.removeItem(key);
  }
}

function commandDatabase(factory: IDBFactory): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const fail = (message: string) => {
      if (settled) return;
      settled = true;
      reject(new CommandLedgerUnavailableError(message));
    };
    const request = factory.open(COMMAND_DATABASE_NAME, COMMAND_DATABASE_VERSION);
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(COMMAND_DATABASE_STORE)) {
        database.createObjectStore(COMMAND_DATABASE_STORE);
      }
    };
    request.onerror = () => {
      fail('This browser could not open the durable command ledger.');
    };
    request.onblocked = () => {
      fail('Another browser context blocked the durable command ledger upgrade.');
    };
    request.onsuccess = () => {
      if (settled) {
        request.result.close();
        return;
      }
      settled = true;
      const database = request.result;
      database.onversionchange = () => database.close();
      resolve(database);
    };
  });
}

function commandRequest<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () =>
      reject(new CommandLedgerUnavailableError('The durable command ledger request failed.'));
  });
}

function commandTransaction(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () =>
      reject(new CommandLedgerUnavailableError('The durable command ledger transaction failed.'));
    transaction.onabort = () =>
      reject(new CommandLedgerUnavailableError('The durable command ledger transaction aborted.'));
  });
}

/**
 * IndexedDB commits are visible across renderer processes before completion.
 * localStorage does not provide that transaction boundary even when its callers
 * are serialized with Web Locks, so it cannot be the production claim store.
 */
class IndexedDbCommandStorage implements CommandStorage {
  readonly #database: Promise<IDBDatabase>;

  constructor(factory: IDBFactory) {
    this.#database = commandDatabase(factory);
  }

  async getItem(key: string): Promise<string | null> {
    const database = await this.#database;
    const transaction = database.transaction(COMMAND_DATABASE_STORE, 'readonly');
    const completion = commandTransaction(transaction);
    const [result] = await Promise.all([
      commandRequest<unknown>(transaction.objectStore(COMMAND_DATABASE_STORE).get(key)),
      completion,
    ]);
    if (result === undefined) return null;
    if (typeof result !== 'string') {
      throw new CommandLedgerUnavailableError('The durable command ledger record is malformed.');
    }
    return result;
  }

  async setItem(key: string, value: string): Promise<void> {
    const database = await this.#database;
    const transaction = database.transaction(COMMAND_DATABASE_STORE, 'readwrite');
    const completion = commandTransaction(transaction);
    await Promise.all([
      commandRequest(transaction.objectStore(COMMAND_DATABASE_STORE).put(value, key)),
      completion,
    ]);
  }

  async removeItem(key: string): Promise<void> {
    const database = await this.#database;
    const transaction = database.transaction(COMMAND_DATABASE_STORE, 'readwrite');
    const completion = commandTransaction(transaction);
    await Promise.all([
      commandRequest(transaction.objectStore(COMMAND_DATABASE_STORE).delete(key)),
      completion,
    ]);
  }
}

function commandStorage(storage: Storage | undefined): CommandStorage {
  if (storage !== undefined) return new SynchronousCommandStorage(storage);
  if (typeof window !== 'undefined' && window.indexedDB !== undefined) {
    return new IndexedDbCommandStorage(window.indexedDB);
  }
  return new SynchronousCommandStorage(browserStorage());
}

const browserCoordinator: CommandCoordinator = {
  async exclusive<T>(name: string, task: () => Promise<T>): Promise<T> {
    if (typeof navigator === 'undefined' || navigator.locks === undefined) {
      throw new CommandLedgerUnavailableError(
        'This browser cannot coordinate commands between CMA tabs.',
      );
    }
    return navigator.locks.request(name, { mode: 'exclusive' }, task);
  },
};

function notifyLedgerChange(storageKey: string): void {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new CustomEvent(COMMAND_LEDGER_EVENT, { detail: storageKey }));
  ledgerBroadcastChannel?.postMessage(storageKey);
}

const ledgerBroadcastChannel =
  typeof window !== 'undefined' && typeof window.BroadcastChannel === 'function'
    ? new window.BroadcastChannel(COMMAND_BROADCAST_CHANNEL)
    : null;

ledgerBroadcastChannel?.addEventListener('message', (event: MessageEvent<unknown>) => {
  if (typeof event.data !== 'string' || !event.data.startsWith(COMMAND_STORAGE_PREFIX)) return;
  window.dispatchEvent(new CustomEvent(COMMAND_LEDGER_EVENT, { detail: event.data }));
});

function hasOwn(value: object, key: PropertyKey): boolean {
  return Object.prototype.hasOwnProperty.call(value, key);
}

async function parseCommand(value: unknown): Promise<StoredCommand> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new CommandLedgerUnavailableError('The retained command record is malformed.');
  }
  const item = value as Record<string, unknown>;
  if (item.version !== VERSION) {
    throw new CommandLedgerUnavailableError('The retained command version is unsupported.');
  }
  const createdAt = checkedTime(item.createdAt, 'creation time');
  const expiresAt = checkedTime(item.expiresAt, 'expiry time');
  if (expiresAt <= createdAt || expiresAt - createdAt > COMMAND_RETENTION_MS) {
    throw new CommandLedgerUnavailableError('The retained command has an invalid lifetime.');
  }
  const leaseHolder = item.leaseHolder;
  const leaseToken = hasOwn(item, 'leaseToken') ? item.leaseToken : null;
  const leaseExpiresAt = item.leaseExpiresAt;
  if (!(
    (leaseHolder === null && leaseToken === null && leaseExpiresAt === null) ||
    (typeof leaseHolder === 'string' &&
      leaseHolder.length > 0 &&
      leaseHolder.length <= 128 &&
      typeof leaseToken === 'string' &&
      leaseToken.length > 0 &&
      leaseToken.length <= 128 &&
      typeof leaseExpiresAt === 'number' &&
      Number.isSafeInteger(leaseExpiresAt) &&
      leaseExpiresAt >= createdAt &&
      leaseExpiresAt <= expiresAt)
  )) {
    throw new CommandLedgerUnavailableError('The retained command has an invalid execution lease.');
  }
  const completedAt = hasOwn(item, 'completedAt')
    ? checkedTime(item.completedAt, 'completion time')
    : undefined;
  if (
    completedAt !== undefined &&
    (completedAt < createdAt ||
      completedAt >= expiresAt ||
      leaseHolder !== null ||
      leaseToken !== null ||
      leaseExpiresAt !== null)
  ) {
    throw new CommandLedgerUnavailableError('The retained command has an invalid completion.');
  }
  const fingerprint = checkedString(item.fingerprint, 'request fingerprint', 64);
  if (!/^[0-9a-f]{64}$/.test(fingerprint)) {
    throw new CommandLedgerUnavailableError('The retained command fingerprint is malformed.');
  }
  const key = checkedString(item.key, 'idempotency key', 255);
  if (!/^[\x21-\x7e]+$/.test(key)) {
    throw new CommandLedgerUnavailableError('The retained idempotency key is malformed.');
  }
  const command: StoredCommand = {
    version: VERSION,
    slot: checkedString(item.slot, 'slot'),
    label: checkedString(item.label, 'label'),
    resource: checkedString(item.resource, 'resource'),
    action: checkedString(item.action, 'action'),
    key,
    fingerprint,
    ...(hasOwn(item, 'request') ? { request: item.request } : {}),
    ...(hasOwn(item, 'retained') ? { retained: item.retained } : {}),
    createdAt,
    expiresAt,
    ...(completedAt === undefined ? {} : { completedAt }),
    leaseHolder,
    leaseToken,
    leaseExpiresAt,
  };
  if (command.request !== undefined) {
    const requestFingerprint = await sha256(canonicalJson(command.request));
    if (requestFingerprint !== command.fingerprint) {
      throw new CommandLedgerUnavailableError(
        'The retained command request does not match its fingerprint.',
      );
    }
  }
  if (command.retained !== undefined) canonicalJson(command.retained);
  return command;
}

/**
 * Durable, owner-scoped browser command identity.
 *
 * Web Locks serialize record claims across same-origin tabs. The HTTP request
 * runs outside that lock under a short stored lease; server idempotency remains
 * the authority, while the lease prevents two healthy tabs from sending at
 * once. A crashed tab relinquishes only the lease, never the command key.
 */
export class BrowserCommandLedger {
  readonly #storage: CommandStorage;
  readonly #coordinator: CommandCoordinator;
  readonly #now: () => number;
  readonly #instanceId: string;
  readonly #runtime = new Map<string, RuntimeCommand>();
  readonly #scope: Promise<{ readonly storageKey: string; readonly lockName: string }>;
  #lastFailure: CommandLedgerUnavailableError | null = null;

  constructor({
    ownerId,
    storage,
    coordinator = browserCoordinator,
    now = Date.now,
    instanceId = crypto.randomUUID(),
  }: BrowserCommandLedgerOptions) {
    const checkedOwner = checkedString(ownerId, 'owner scope', 2048);
    this.#storage = commandStorage(storage);
    this.#coordinator = coordinator;
    this.#now = now;
    this.#instanceId = checkedString(instanceId, 'browser instance', 128);
    this.#scope = sha256(checkedOwner).then((ownerHash) => ({
      storageKey: `${COMMAND_STORAGE_PREFIX}${ownerHash}`,
      lockName: `${COMMAND_STORAGE_PREFIX}${ownerHash}`,
    }));
  }

  async execute<TRequest, TResult>(
    intent: BrowserCommandIntent<TRequest>,
    label: string,
    send: (key: string, request: TRequest) => Promise<TResult>,
  ): Promise<TResult> {
    const prepared = await this.#prepare(intent);
    const stored = await this.#claim(prepared, label);
    let runtime = this.#runtime.get(stored.key);
    if (runtime === undefined) {
      const request = (stored.request === undefined ? intent.request : stored.request) as TRequest;
      runtime = {
        fingerprint: stored.fingerprint,
        request,
        send: (key, value) => send(key, value as TRequest),
        inFlight: null,
      };
      this.#runtime.set(stored.key, runtime);
    } else if (runtime.fingerprint !== prepared.fingerprint) {
      throw new CommandDescriptorMismatchError();
    }
    return this.#attempt<TResult>(stored, runtime);
  }

  async pending(resources: readonly string[]): Promise<PendingBrowserCommand | null> {
    const wanted = new Set(resources);
    if (wanted.size === 0) return null;
    const scope = await this.#scope;
    return this.#coordinator.exclusive(scope.lockName, async () => {
      const commands = await this.#load(scope.storageKey);
      const command = commands
        .filter((item) => item.completedAt === undefined && wanted.has(item.resource))
        .sort((left, right) => left.createdAt - right.createdAt)[0];
      return command === undefined ? null : this.#public(command);
    });
  }

  async has(resource: string, action?: string): Promise<boolean> {
    const scope = await this.#scope;
    return this.#coordinator.exclusive(scope.lockName, async () => {
      const commands = await this.#load(scope.storageKey);
      return commands.some(
        (item) =>
          item.completedAt === undefined &&
          item.resource === resource &&
          (action === undefined || item.action === action),
      );
    });
  }

  /** Return and clear a non-authoritative cleanup failure from the last command attempt. */
  takeFailure(): CommandLedgerUnavailableError | null {
    const failure = this.#lastFailure;
    this.#lastFailure = null;
    return failure;
  }

  async #prepare<TRequest>(
    intent: BrowserCommandIntent<TRequest>,
  ): Promise<PreparedIntent<TRequest>> {
    checkedString(intent.slot, 'slot');
    checkedString(intent.resource, 'resource');
    checkedString(intent.action, 'action');
    const canonicalRequest = canonicalJson(intent.request);
    const storedRequest = intent.persistRequest
      ? (JSON.parse(canonicalRequest) as unknown)
      : undefined;
    const storedRetained =
      intent.retained === undefined
        ? undefined
        : (JSON.parse(canonicalJson(intent.retained)) as unknown);
    return {
      intent,
      fingerprint: await sha256(canonicalRequest),
      ...(storedRequest === undefined ? {} : { storedRequest }),
      ...(storedRetained === undefined ? {} : { storedRetained }),
    };
  }

  async #claim<TRequest>(
    prepared: PreparedIntent<TRequest>,
    label: string,
  ): Promise<StoredCommand> {
    checkedString(label, 'label');
    const scope = await this.#scope;
    return this.#coordinator.exclusive(scope.lockName, async () => {
      const now = this.#now();
      const commands = await this.#load(scope.storageKey);
      let available = commands;
      let sameResource = commands.find((command) => command.resource === prepared.intent.resource);
      if (sameResource?.completedAt !== undefined) {
        const replaysExactCompletedCommand =
          prepared.intent.replayCompleted === true &&
          sameResource.slot === prepared.intent.slot &&
          sameResource.action === prepared.intent.action &&
          sameResource.fingerprint === prepared.fingerprint;
        if (replaysExactCompletedCommand) {
          // One-shot commands such as Start must converge even when another
          // tab reaches the ledger just after the winning response completed.
          sameResource = { ...sameResource, completedAt: undefined };
        } else {
          // Completion settles the old user intent. Repeatable actions allocate
          // a fresh key unless their caller explicitly opts into exact replay.
          available = commands.filter((item) => item.key !== sameResource?.key);
          sameResource = undefined;
        }
      }
      let command: StoredCommand;
      if (sameResource === undefined) {
        command = {
          version: VERSION,
          slot: prepared.intent.slot,
          label,
          resource: prepared.intent.resource,
          action: prepared.intent.action,
          key: newIntentKey(),
          fingerprint: prepared.fingerprint,
          ...(prepared.storedRequest === undefined ? {} : { request: prepared.storedRequest }),
          ...(prepared.storedRetained === undefined ? {} : { retained: prepared.storedRetained }),
          createdAt: now,
          expiresAt: now + COMMAND_RETENTION_MS,
          leaseHolder: null,
          leaseToken: null,
          leaseExpiresAt: null,
        };
      } else {
        if (
          sameResource.slot !== prepared.intent.slot ||
          sameResource.action !== prepared.intent.action ||
          sameResource.fingerprint !== prepared.fingerprint
        ) {
          if (
            sameResource.slot === prepared.intent.slot &&
            sameResource.action === prepared.intent.action
          ) {
            if (sameResource.request === undefined && !this.#runtime.has(sameResource.key)) {
              throw new CommandRequestUnavailableError();
            }
            throw new CommandDescriptorMismatchError();
          }
          throw new CommandResourceBusyError(sameResource.label);
        }
        command = sameResource;
      }

      if (
        command.leaseHolder !== null &&
        command.leaseHolder !== this.#instanceId &&
        command.leaseExpiresAt !== null &&
        command.leaseExpiresAt > now
      ) {
        throw new CommandExecutionClaimedError();
      }

      if (
        command.leaseHolder === this.#instanceId &&
        command.leaseToken !== null &&
        command.leaseExpiresAt !== null &&
        (command.leaseExpiresAt > now ||
          (this.#runtime.get(command.key)?.inFlight ?? null) !== null)
      ) {
        // Reuse the exact local claim both during the short interval before
        // runtime registration and while its request is still running. A
        // rotation followed by joining that promise would leave no completion
        // authorized to clear the replacement token.
        return command;
      }

      const claimed: StoredCommand = {
        ...command,
        leaseHolder: this.#instanceId,
        leaseToken: crypto.randomUUID(),
        leaseExpiresAt: Math.min(now + COMMAND_LEASE_MS, command.expiresAt),
      };
      await this.#save(
        scope.storageKey,
        sameResource === undefined
          ? [...available, claimed]
          : available.map((item) => (item.key === claimed.key ? claimed : item)),
      );
      return claimed;
    });
  }

  async #attempt<TResult>(stored: StoredCommand, runtime: RuntimeCommand): Promise<TResult> {
    if (runtime.inFlight !== null) return runtime.inFlight as Promise<TResult>;
    const send = Promise.resolve().then(() => runtime.send(stored.key, runtime.request));
    const attempt = (async (): Promise<TResult> => {
      let result: unknown;
      try {
        result = await send;
      } catch (cause) {
        try {
          await this.#finish(stored, commandOutcomeIsUnresolved(cause) ? 'retain' : 'remove');
        } catch (cleanupCause) {
          // The HTTP outcome remains authoritative. Leaving the same-key record
          // behind is fail-closed and lets a later read reconcile browser state.
          this.#rememberFailure(cleanupCause);
        }
        throw cause;
      }
      try {
        await this.#finish(stored, 'complete');
      } catch (cleanupCause) {
        // Do not turn a successful server command into a second logical intent.
        // A retained record is safer than erasing the only durable command key.
        this.#rememberFailure(cleanupCause);
      }
      return result as TResult;
    })().finally(() => {
      runtime.inFlight = null;
    });
    runtime.inFlight = attempt;
    return attempt;
  }

  #rememberFailure(cause: unknown): void {
    this.#lastFailure =
      cause instanceof CommandLedgerUnavailableError
        ? cause
        : new CommandLedgerUnavailableError(
            cause instanceof Error ? cause.message : 'The command ledger cleanup failed.',
          );
  }

  async #finish(
    claim: StoredCommand,
    disposition: 'complete' | 'remove' | 'retain',
  ): Promise<void> {
    const scope = await this.#scope;
    await this.#coordinator.exclusive(scope.lockName, async () => {
      const commands = await this.#load(scope.storageKey);
      const current = commands.find((item) => item.key === claim.key);
      if (current === undefined) {
        this.#runtime.delete(claim.key);
        return;
      }
      if (
        current.leaseHolder !== claim.leaseHolder ||
        current.leaseToken !== claim.leaseToken ||
        current.leaseExpiresAt !== claim.leaseExpiresAt
      ) {
        return;
      }
      const next =
        disposition === 'remove'
          ? commands.filter((item) => item.key !== claim.key)
          : commands.map((item) =>
              item.key !== claim.key
                ? item
                : disposition === 'complete'
                  ? {
                      ...item,
                      completedAt: this.#now(),
                      leaseHolder: null,
                      leaseToken: null,
                      leaseExpiresAt: null,
                    }
                  : { ...item, leaseHolder: null, leaseToken: null, leaseExpiresAt: null },
            );
      await this.#save(scope.storageKey, next);
      if (disposition !== 'retain') this.#runtime.delete(claim.key);
    });
  }

  async #load(storageKey: string): Promise<readonly StoredCommand[]> {
    let raw: string | null;
    try {
      raw = await this.#storage.getItem(storageKey);
    } catch (cause) {
      throw new CommandLedgerUnavailableError(
        cause instanceof Error
          ? `This browser could not read retained commands: ${cause.message}`
          : 'This browser could not read retained commands.',
      );
    }
    if (raw === null) return [];
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw) as unknown;
    } catch {
      throw new CommandLedgerUnavailableError('The retained command ledger is malformed.');
    }
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      throw new CommandLedgerUnavailableError('The retained command ledger is malformed.');
    }
    const envelope = parsed as Record<string, unknown>;
    if (envelope.version !== VERSION || !Array.isArray(envelope.commands)) {
      throw new CommandLedgerUnavailableError(
        'The retained command ledger version is unsupported.',
      );
    }
    const commands = await Promise.all(envelope.commands.map((item) => parseCommand(item)));
    const resources = new Set<string>();
    for (const command of commands) {
      if (resources.has(command.resource)) {
        throw new CommandLedgerUnavailableError(
          'The retained command ledger contains competing actions for one resource.',
        );
      }
      resources.add(command.resource);
    }
    const now = this.#now();
    const live = commands.filter((command) => command.expiresAt > now);
    if (live.length !== commands.length) await this.#save(storageKey, live);
    return live;
  }

  async #save(storageKey: string, commands: readonly StoredCommand[]): Promise<void> {
    try {
      if (commands.length === 0) {
        await this.#storage.removeItem(storageKey);
      } else {
        const envelope: StoredEnvelope = { version: VERSION, commands };
        await this.#storage.setItem(storageKey, JSON.stringify(envelope));
      }
    } catch (cause) {
      throw new CommandLedgerUnavailableError(
        cause instanceof Error
          ? `This browser could not retain the command: ${cause.message}`
          : 'This browser could not retain the command.',
      );
    }
    notifyLedgerChange(storageKey);
  }

  #public(command: StoredCommand): PendingBrowserCommand {
    return {
      slot: command.slot,
      label: command.label,
      resource: command.resource,
      action: command.action,
      ...(command.request === undefined ? {} : { request: command.request }),
      ...(command.retained === undefined ? {} : { retained: command.retained }),
      requiresRequestReconstruction:
        command.request === undefined && !this.#runtime.has(command.key),
      expiresAt: command.expiresAt,
    };
  }
}
