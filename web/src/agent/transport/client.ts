import type { Cma, V2ResponseReceipt, AnswerContext } from '@chronoai/cma-sdk';
import type { ReadOptions } from '../../api/client';
import {
  agentTransportJournal,
  guardAgentTransportIdentity,
  type AgentTransportJournal,
} from './journal';
import { IdentityChangedError } from './identity';
import { recoverAgentInput, submitAgentInput } from './submit';
import { createTransportPreparation } from './preparation';
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
}

export class AgentV2Session {
  readonly events: Cma['agentEventsV2'];
  readonly responses: Cma['responsesV2'];
  private entryHandoff?: () => boolean;

  constructor(
    private readonly sdk: Cma,
    readonly journal: AgentTransportJournal,
    private readonly user: string,
    private readonly options: AgentTransportOptions,
  ) {
    this.events = sdk.agentEventsV2;
    this.responses = sdk.responsesV2;
  }

  get owner(): string {
    return this.user;
  }

  /** A fresh replay handle keeps the tab's selection and pending-input journal. */
  forSnapshotRefresh(): AgentV2Session {
    return new AgentV2Session(this.sdk, this.journal, this.user, this.options);
  }

  async authorize(signal?: AbortSignal): Promise<void> {
    this.discardEntryHandoff();
    signal?.throwIfAborted();
    this.journal.read();
    const current = await this.options.currentUser(signal);
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
  ) {
    await this.authorize(signal);
    return submitAgentInput(this.sdk, this.journal, input, {
      onAccepted,
      onReceiptObserved,
      answerContext,
      signal,
      authorize: () => this.authorize(signal),
    });
  }

  async recover(onAccepted: (receipt: V2ResponseReceipt) => void, signal?: AbortSignal) {
    await this.authorize(signal);
    return recoverAgentInput(this.sdk, this.journal, {
      onAccepted,
      signal,
      authorize: () => this.authorize(signal),
    });
  }
}

/** The authenticated server flag selects a new tab; retained v2 work keeps its transport. */
export function createAgentTransport(sdk: Cma, options: AgentTransportOptions) {
  const gate = createIdentityGate(options.currentUser);
  const gatedOptions = { ...options, currentUser: gate.check };
  const sessions = new Map<string, AgentV2Session>();
  const select = async (
    agent: string,
    read: ReadOptions = {},
    expectedOwner?: string,
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
    const session = new AgentV2Session(sdk, journal, user, gatedOptions);
    let selected = journal.selected();
    let enabled = false;
    if (selected === undefined) {
      try {
        const eligibility = await sdk.agentEventsV2.readTransportEligibility(read);
        enabled = eligibility.enabled && eligibility.live_fragments;
      } catch {
        read.signal?.throwIfAborted();
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
  return { open, legacySession, ...createTransportPreparation(select, open) };
}
