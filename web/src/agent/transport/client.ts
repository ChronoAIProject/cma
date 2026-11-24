import { ApiError, type Cma, type V2ResponseReceipt, type AnswerContext } from '@cma/client';
import type { ReadOptions } from '../../api/client';
import {
  agentTransportJournal,
  guardAgentTransportIdentity,
  type AgentTransportJournal,
} from './journal';
import { IdentityChangedError } from './identity';
import { recoverAgentInput, submitAgentInput } from './submit';
import { createTransportPreparation } from './preparation';
import { withRequestRecovery, type RecoveryPolicy, type RequestRecovery } from './requestRecovery';
import {
  createIdentityGate,
  createIdentityHandoff,
  IDENTITY_HANDOFF_ENABLED,
  type CurrentUser,
} from './identityGate';

export interface AgentTransportOptions {
  readonly storage: Storage;
  /** Recheck the authenticated account before requests and before exposing reads. */
  readonly currentUser: CurrentUser;
  /** Replica recovery is opt-in; the browser build supplies the approved policy. */
  readonly recovery?: RecoveryPolicy;
}

export class AgentV2Session {
  private approvalProjectionInvalidated = false;

  invalidateApprovalProjection() {
    this.approvalProjectionInvalidated = true;
  }
  get approvalProjectionAvailable() {
    return !this.approvalProjectionInvalidated;
  }

  readonly events: Cma['agentEventsV2'];
  readonly responses: Cma['responsesV2'];
  private entryHandoff?: () => boolean;
  private readonly admittedInputs = new Map<
    string,
    { receipt: V2ResponseReceipt; input: string }
  >();

  recordAcceptedInput(receipt: V2ResponseReceipt, input: string): void {
    this.admittedInputs.set(receipt.commandId, { receipt, input });
    try {
      this.journal.queuedInputs.remember([receipt.commandId]);
    } catch {
      // Status reconciliation retains the in-memory receipt and retries persistence.
    }
  }

  knownInputs() {
    return this.admittedInputs.values();
  }

  constructor(
    private readonly sdk: Cma,
    readonly journal: AgentTransportJournal,
    private readonly user: string,
    private readonly options: AgentTransportOptions,
    private readonly cancelUser: CurrentUser = options.currentUser,
    private readonly inputUser: CurrentUser = options.currentUser,
  ) {
    this.events = sdk.agentEventsV2;
    this.responses = sdk.responsesV2;
  }

  get owner(): string {
    return this.user;
  }

  /** A fresh replay handle keeps the tab's selection and pending-input journal. */
  forSnapshotRefresh(): AgentV2Session {
    const refreshed = new AgentV2Session(
      this.sdk,
      this.journal,
      this.user,
      this.options,
      this.cancelUser,
      this.inputUser,
    );
    for (const [id, value] of this.admittedInputs) refreshed.admittedInputs.set(id, value);
    return refreshed;
  }

  async authorize(signal?: AbortSignal): Promise<void> {
    return this.checkIdentity(this.options.currentUser, signal);
  }

  /** Cancel must not wait for an unrelated projection's identity request. */
  async authorizeCancel(signal?: AbortSignal): Promise<void> {
    return this.checkIdentity(this.cancelUser, signal);
  }

  private async checkIdentity(currentUser: CurrentUser, signal?: AbortSignal): Promise<void> {
    this.discardEntryHandoff();
    signal?.throwIfAborted();
    this.journal.read();
    const current = await currentUser(signal);
    signal?.throwIfAborted();
    this.journal.read();
    if (current !== this.user) {
      this.journal.invalidate();
      throw new IdentityChangedError();
    }
  }

  retainEntryHandoff(): void {
    this.entryHandoff = createIdentityHandoff(IDENTITY_HANDOFF_ENABLED);
  }

  discardEntryHandoff(): void {
    this.entryHandoff = undefined;
  }

  async authorizeLoad(signal: AbortSignal): Promise<void> {
    signal.throwIfAborted();
    this.journal.read();
    const handoff = this.entryHandoff;
    this.discardEntryHandoff();
    if (!handoff?.()) await this.authorize(signal);
    signal.throwIfAborted();
    this.journal.read();
  }

  pending(): {
    readonly input?: string;
    readonly accepted?: string;
    readonly acceptedSteering?: string;
    readonly acceptedInterruption?: string;
    readonly control?: boolean;
  } {
    try {
      const input = this.journal.read();
      const control = this.journal.readControl();
      const interruption = this.journal.interruption.read();
      return {
        accepted: input?.state === 'accepted' ? input.input : undefined,
        acceptedSteering:
          control?.commandId != null && control.body.command.type === 'steer'
            ? control.body.command.text
            : undefined,
        input: input?.state === 'pending' ? input.input : undefined,
        acceptedInterruption: interruption?.receipt != null ? interruption.body.text : undefined,
        control: control?.commandId === null || interruption?.receipt === null,
      };
    } catch {
      // A stale view may finish rendering while sign-out invalidates its journal.
      // Every mutation still rechecks identity and refuses a stale handle.
      return {};
    }
  }

  async send(
    input: string,
    onAccepted: (receipt: V2ResponseReceipt) => void,
    signal?: AbortSignal,
    onReceiptObserved?: (receipt: V2ResponseReceipt) => void,
    answerContext?: AnswerContext,
    onRetained?: (key: string) => void,
  ) {
    return this.withRecovery(signal, async (recovery) => {
      await recovery.retry(() => this.checkIdentity(this.inputUser, recovery.signal));
      return submitAgentInput(this.sdk, this.journal, input, {
        onAccepted: (receipt) => {
          this.recordAcceptedInput(receipt, input);
          onAccepted(receipt);
        },
        onReceiptObserved,
        onRetained,
        answerContext,
        signal: recovery.signal,
        recovery,
        authorize: () => this.checkIdentity(this.inputUser, recovery.signal),
      });
    });
  }

  async recover(onAccepted: (receipt: V2ResponseReceipt) => void, signal?: AbortSignal) {
    return this.withRecovery(signal, async (recovery) => {
      await recovery.retry(() => this.checkIdentity(this.inputUser, recovery.signal));
      const input = this.journal.read()?.input;
      return recoverAgentInput(this.sdk, this.journal, {
        onAccepted: (receipt) => {
          if (input !== undefined) this.recordAcceptedInput(receipt, input);
          onAccepted(receipt);
        },
        signal: recovery.signal,
        recovery,
        authorize: () => this.checkIdentity(this.inputUser, recovery.signal),
      });
    });
  }

  async recoverLegacySuggestion(signal?: AbortSignal) {
    return this.withRecovery(signal, async (recovery) => {
      await recovery.retry(() => this.checkIdentity(this.inputUser, recovery.signal));
      const input = this.journal.legacySuggestion.read();
      if (!input || input.state !== 'pending') return input?.receipt?.runId;
      const receipt = await recovery.retry(() =>
        this.sdk.agui.findInputReceipt(
          {
            threadId: this.journal.agentId,
            runId: input.key,
            forwardedProps: { answerContext: input.answerContext },
            messages: [{ role: 'user', content: input.input }],
          },
          {
            idempotencyKey: input.key,
            signal: recovery.signal,
            timeoutMs: recovery.timeoutMs ?? 5000,
          },
        ),
      );
      if (!receipt?.runId) return undefined;
      await recovery.retry(() => this.checkIdentity(this.inputUser, recovery.signal));
      recovery.signal.throwIfAborted();
      this.journal.legacySuggestion.accepted(input.key, receipt.runId);
      return receipt.runId;
    });
  }

  withRecovery<T>(
    signal: AbortSignal | undefined,
    operation: (context: RequestRecovery) => Promise<T>,
    eligible = true,
  ) {
    return withRequestRecovery(
      signal,
      eligible && this.options.recovery
        ? this.options.recovery
        : { enabled: false, delay: () => 0 },
      operation,
    );
  }
}

/** The authenticated server flag selects a new tab; retained v2 work keeps its transport. */
export function createAgentTransport(sdk: Cma, options: AgentTransportOptions) {
  const gate = createIdentityGate(options.currentUser);
  // Shared across every Agent: at most one extra fresh identity read, even when
  // many tabs cancel concurrently. All gates retain the same journal authority.
  const cancelGate = createIdentityGate(options.currentUser);
  // Send and receipt/recovery checks cannot wait for background observation.
  // Keep Cancel independent too: three bounded gates, never one per Agent.
  const inputGate = createIdentityGate(options.currentUser);
  const gatedOptions = { ...options, currentUser: gate.check };
  const sessions = new Map<string, AgentV2Session>();
  const select = async (
    agent: string,
    read: ReadOptions = {},
    expectedOwner?: string,
    onIdentity?: (session: AgentV2Session) => void,
  ): Promise<AgentV2Session | undefined> => {
    read.signal?.throwIfAborted();
    const checkIdentity =
      expectedOwner === undefined ? undefined : guardAgentTransportIdentity(options.storage);
    const user = await gate.check(read.signal);
    read.signal?.throwIfAborted();
    checkIdentity?.();
    if (expectedOwner !== undefined && user !== expectedOwner) throw new IdentityChangedError();
    if (user === undefined) throw new Error('Sign in to open this Agent.');
    const journal = agentTransportJournal(options.storage, user, agent);
    const session = new AgentV2Session(
      sdk,
      journal,
      user,
      gatedOptions,
      cancelGate.check,
      inputGate.check,
    );
    onIdentity?.(session);
    let selected = journal.selected();
    let enabled = false;
    if (selected === undefined) {
      try {
        const eligibility = await sdk.agentEventsV2.readTransportEligibility(read);
        enabled = eligibility.enabled && eligibility.live_fragments;
      } catch (error) {
        read.signal?.throwIfAborted();
        // A throttled selection must remain retryable, never pin legacy transport.
        if (error instanceof ApiError && (error.status === 429 || error.retryAfterMs !== undefined))
          throw error;
        // Older or unavailable servers cannot select a new v2 tab.
      }
    }
    // Eligibility crosses an IO boundary, so only an existing v2 selection can
    // reuse the immediately preceding account check. Legacy semantics stay intact.
    if (!IDENTITY_HANDOFF_ENABLED || selected !== 'responses_v2')
      await session.authorize(read.signal);
    selected ??= journal.select(enabled);
    if (selected === 'responses_v2') session.retainEntryHandoff();
    sessions.set(agent, session);
    return selected === 'responses_v2' ? session : undefined;
  };
  const open = (agent: string, read: ReadOptions = {}) => select(agent, read);
  const legacySession = (agent: string | undefined, owner: string | undefined) => {
    const session = agent === undefined ? undefined : sessions.get(agent);
    if (session?.owner !== owner) return undefined;
    try {
      return session?.journal.selected() === 'agui' ? session : undefined;
    } catch {
      return undefined;
    }
  };
  return {
    open,
    legacySession,
    identityFence: () => guardAgentTransportIdentity(options.storage),
    ...createTransportPreparation(select, open),
  };
}
