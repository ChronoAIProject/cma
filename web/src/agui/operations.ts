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

/** Why an operation could not be performed. */
export class AguiRefusal extends Error {
  constructor(
    /** The control plane's own machine-readable code. */
    readonly code: string,
    message: string,
    /**
     * The ability this session lacks, when that is why.
     *
     * Present on a capability refusal and absent otherwise, which is the
     * distinction the whole capability model exists to preserve: "this session
     * can never do that" is not "that just failed", and only the second is
     * worth retrying.
     */
    readonly capability?: string,
  ) {
    super(message);
    this.name = 'AguiRefusal';
  }

  /** Whether this is a permanent absence rather than a transient failure. */
  get isCapability(): boolean {
    return this.capability !== undefined;
  }
}

/** What a session was granted, read once so components can ask rather than try. */
export interface SessionCapabilities {
  readonly conversation: boolean;
  readonly terminal: boolean;
  readonly steer: boolean;
  readonly interrupt: boolean;
  readonly usage: boolean;
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
    steer: holds('steer'),
    interrupt: holds('interrupt'),
    usage: holds('usage'),
    concurrentInput: POLICIES.find((known) => known === policy) ?? 'reject',
  };
}

/** How the operations reach the control plane. Swapped out by the mock driver. */
export interface OperationTransport {
  send(path: string, body: unknown): Promise<Response>;
}

/** The default transport: same-origin, cookie-authenticated, like `api/client`. */
export const fetchTransport: OperationTransport = {
  send: (path, body) =>
    fetch(path, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    }),
};

/**
 * Turn a refusal into one a caller can act on.
 *
 * The control plane answers RFC 9457 problem documents carrying `code` and
 * `detail`; both travel. A body that cannot be read still produces a refusal
 * naming the status, because a caller has to be told something true.
 */
async function refusalFrom(response: Response): Promise<AguiRefusal> {
  let code = `http_${String(response.status)}`;
  let detail = `the control plane refused with ${String(response.status)}`;
  let capability: string | undefined;
  try {
    const body = (await response.json()) as Record<string, unknown>;
    if (typeof body.code === 'string') {
      code = body.code;
    }
    if (typeof body.detail === 'string') {
      detail = body.detail;
    }
    if (code === 'capability_unsupported') {
      // The detail is "capability not supported: <name>". The name is the
      // part a person can act on, so it is lifted out rather than left in
      // prose a component would have to parse.
      capability = detail.split(':').pop()?.trim();
    }
  } catch {
    // Not a problem document. The status is still true.
  }
  return new AguiRefusal(code, detail, capability);
}

export class AguiOperations {
  constructor(
    private readonly session: string,
    private readonly transport: OperationTransport = fetchTransport,
  ) {}

  /** Open a turn with a prompt. */
  async prompt(text: string, runId: string): Promise<void> {
    await this.#post('/api/v1/agui/run', {
      threadId: this.session,
      runId,
      messages: [{ id: runId, role: 'user', content: text }],
    });
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
  async resolveApproval(interruptId: string, decision: string, runId: string): Promise<void> {
    await this.#post('/api/v1/agui/run', {
      threadId: this.session,
      runId,
      resume: [{ interruptId, status: 'resolved', payload: { decision } }],
    });
  }

  /**
   * Stop the turn in flight.
   *
   * Its own route, and not a dropped stream: `docs/API_AGUI.md` §5 is explicit
   * that a dropped stream is a client that stopped watching, and that reading
   * it as a cancellation would end somebody's turn because their network
   * blinked.
   */
  async interrupt(): Promise<void> {
    await this.#post(`/api/v1/sessions/${encodeURIComponent(this.session)}/interrupt`, {});
  }

  /**
   * Send a turn while one is in flight.
   *
   * What happens is the session's `concurrent_input` policy, not this client's
   * choice. On `reject` the control plane answers 409 before opening a stream,
   * and that refusal reaches the caller with its code intact.
   */
  async steer(text: string, runId: string): Promise<void> {
    await this.prompt(text, runId);
  }

  async #post(path: string, body: unknown): Promise<void> {
    const response = await this.transport.send(path, body);
    if (!response.ok) {
      throw await refusalFrom(response);
    }
  }
}
