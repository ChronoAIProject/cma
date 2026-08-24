/**
 * What a conversation client can ask a session to do.
 *
 * Each operation returns the control plane's **typed refusal unchanged** rather
 * than throwing a generic failure. `API_Overview.md` rule 4: an error names
 * what is missing — and the interface is going to show that name, so losing it
 * here would be losing the only part a person can act on.
 *
 * No React. This is a protocol client.
 */

import { resolveConfig, type ClientConfig, type ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import {
  errorFromResponse,
  requestResponse,
  type OperationOptions,
  type RequestOptions,
} from '../http.js';

/** What a session was granted, read once so components can ask rather than try. */
export interface SessionCapabilities {
  readonly conversation: boolean;
  readonly terminal: boolean;
  readonly steer: boolean;
  readonly interrupt: boolean;
  readonly usage: boolean;
  readonly resources: boolean;
  /**
   * What happens when a turn arrives while one is in flight.
   *
   * `API_Overview.md` §6 rule 5: selected per session at creation and reported
   * in its capability document. The composer reads it; it never guesses.
   */
  readonly concurrentInput: ConcurrentInput;
}

export type ConcurrentInput = 'reject' | 'enqueue' | 'interrupt' | 'rollback';

const POLICIES: readonly ConcurrentInput[] = ['reject', 'enqueue', 'interrupt', 'rollback'];

/**
 * A session that was granted nothing.
 *
 * The value `toCapabilities` produces for a payload that declared none, named
 * so a fixture can say "this session holds nothing" rather than spelling six
 * falses out — and so the two can never drift apart.
 */
export const NO_CAPABILITIES: SessionCapabilities = {
  conversation: false,
  terminal: false,
  steer: false,
  interrupt: false,
  usage: false,
  resources: false,
  concurrentInput: 'reject',
};

/**
 * Read a session's granted set.
 *
 * Failing closed: anything the control plane did not clearly grant is not
 * granted. The worst outcome is a control that is not offered; the alternative
 * is offering one the API will refuse.
 */
export function toCapabilities(raw: unknown): SessionCapabilities {
  const value = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  const declared = Array.isArray(value.conforms_to)
    ? value.conforms_to.filter((entry): entry is string => typeof entry === 'string')
    : [];
  const holds = (name: string) => declared.some((uri) => uri.endsWith(`/conf/${name}`));
  const policy = value.concurrent_input;

  return {
    conversation: holds('conversation'),
    terminal: holds('terminal'),
    // The core ability is named `steer`; its conformance class is `steering`.
    // This spelling is fixed by rest/mod.rs::conformance_for and API_Overview.
    steer: holds('steering'),
    interrupt: holds('interrupt'),
    usage: holds('usage'),
    resources: holds('resources'),
    concurrentInput: POLICIES.find((known) => known === policy) ?? 'reject',
  };
}

/**
 * How operations reach the control plane. Swapped out by the mock driver.
 *
 * The caller consumes a failed response as a problem document and cancels a
 * successful run response after validating its headers. Custom transports must
 * therefore return a real `Response` with either a null or cancellable body.
 */
export interface OperationTransport {
  send(path: string, body: unknown, options?: RequestOptions): Promise<Response>;
}

/** Build a fetch transport using the package's shared auth, timeout, and errors. */
export function createOperationTransport(config: ResolvedConfig): OperationTransport {
  return {
    send: (path, body, options = {}) =>
      requestResponse(config, path, { ...options, method: 'POST', body }),
  };
}

/** Same-origin cookie transport retained for the reference web client. */
export const fetchTransport = createOperationTransport(resolveConfig());

export class AguiOperations {
  constructor(
    private readonly session: string,
    transportOrConfig: OperationTransport | ClientConfig = fetchTransport,
  ) {
    this.transport = isTransport(transportOrConfig)
      ? transportOrConfig
      : createOperationTransport(resolveConfig(transportOrConfig));
  }

  private readonly transport: OperationTransport;

  /** Open a turn with a prompt. */
  async prompt(text: string, runId: string, options: OperationOptions = {}): Promise<void> {
    requireText(text, 'prompt');
    requireIdentifier(runId, 'runId');
    await this.#post(
      '/api/v1/agui/run',
      {
        threadId: this.session,
        runId,
        messages: [{ id: runId, role: 'user', content: text }],
      },
      options,
      200,
      'text/event-stream',
    );
  }

  /**
   * Answer an approval.
   *
   * `docs/API_AGUI.md` §4: this is a **new run** on the same thread carrying a
   * `resume`, not a reply on the stream that asked. The decision is the one
   * the kernel offered, passed through unchanged — a value this client
   * invented would be read as a refusal, which is the one direction the
   * service will safely fail in.
   */
  async resolveApproval(
    interruptId: string,
    decision: string,
    runId: string,
    options: OperationOptions = {},
  ): Promise<void> {
    requireIdentifier(interruptId, 'interruptId');
    requireIdentifier(runId, 'runId');
    if (!['allow', 'deny', 'abort'].includes(decision)) {
      throw new ValidationError("approval decision must be 'allow', 'deny', or 'abort'");
    }
    await this.#post(
      '/api/v1/agui/run',
      {
        threadId: this.session,
        runId,
        resume: [{ interruptId, status: 'resolved', payload: { decision } }],
      },
      options,
      200,
      'text/event-stream',
    );
  }

  /**
   * Stop the turn in flight.
   *
   * Its own route, and not a dropped stream: `docs/API_AGUI.md` §5 is explicit
   * that a dropped stream is a client that stopped watching, and that reading
   * it as a cancellation would end somebody's turn because their network
   * blinked.
   */
  async interrupt(options: OperationOptions = {}): Promise<void> {
    await this.#post(
      `/api/v1/sessions/${encodeURIComponent(this.session)}/interrupt`,
      undefined,
      options,
      202,
    );
  }

  /**
   * Send a turn while one is in flight.
   *
   * What happens is the session's `concurrent_input` policy, not this client's
   * choice. On `reject` the control plane answers 409 before opening a stream,
   * and that refusal reaches the caller with its code intact.
   */
  async steer(text: string, runId: string, options: OperationOptions = {}): Promise<void> {
    await this.prompt(text, runId, options);
  }

  async #post(
    path: string,
    body: unknown,
    options: OperationOptions,
    expectedStatus: number,
    expectedContentType?: string,
  ): Promise<void> {
    const response = await this.transport.send(path, body, options);
    if (!response.ok) {
      throw await errorFromResponse(response);
    }
    try {
      if (response.status !== expectedStatus) {
        throw new ProtocolError(
          `AG-UI operation expected HTTP ${String(expectedStatus)} but received ${String(response.status)}`,
        );
      }
      if (expectedContentType !== undefined) {
        const contentType = response.headers.get('content-type') ?? '';
        if (contentType.split(';', 1)[0]?.trim().toLowerCase() !== expectedContentType) {
          throw new ProtocolError(`AG-UI run returned content type '${contentType}'`);
        }
      }
    } finally {
      // API_AGUI.md §7 makes the durable session stream the watcher, and §5
      // says dropping this redundant run stream does not cancel the turn. Every
      // successful HTTP response is released, including a malformed success.
      try {
        await response.body?.cancel();
      } catch {
        // A body that cannot be cancelled is already unusable, which releases
        // it just as surely as cancellation. An accepted run remains active.
      }
    }
  }
}

function isTransport(value: OperationTransport | ClientConfig): value is OperationTransport {
  return typeof (value as Partial<OperationTransport>).send === 'function';
}

function requireIdentifier(value: string, name: string): void {
  if (typeof value !== 'string' || value === '') {
    throw new ValidationError(`${name} must be a non-empty string`);
  }
}

function requireText(value: string, name: string): void {
  if (typeof value !== 'string') {
    throw new ValidationError(`${name} must be a string`);
  }
}
