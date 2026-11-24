import { parseRecord, type InputRecord, type TransportRecord } from './journalRecord';
import type { ControlRecord } from './controlRecord';
import { IdentityChangedError } from './identity';
import type { AnswerContext } from '@cma/client';
import { QueuedCancellationJournal } from './queuedJournal';
import { QueuedInputJournal } from './queuedInputJournal';
import { InterruptionJournal } from './interruptionJournal';
import { LegacySuggestionJournal } from './legacySuggestionJournal';
import { bindDraftCommand, DraftBindingError } from '../draftDelivery';
import { acceptDraftCommand, clearOwnerDrafts } from '../draftStorage';
import {
  promptIntentJournal,
  settingsIntentJournal,
  type RetainedSettingsEditor,
} from '../settingsIntent';

const PREFIX = 'cma:agent-transport:v1:';
const IDENTITY = `${PREFIX}identity`;
type BrowserStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem' | 'key' | 'length'>;
export type AgentTransport = 'agui' | 'responses_v2';

function storageFailure(): Error {
  return new Error(
    'Your pending message could not be recovered. Restore browser storage before sending.',
  );
}

function readIdentity(storage: BrowserStorage): { user: string; epoch: string } | undefined {
  const source = storage.getItem(IDENTITY);
  if (source === null) return undefined;
  const value: unknown = JSON.parse(source);
  if (typeof value !== 'object' || value === null) throw storageFailure();
  const row = value as Record<string, unknown>;
  if (
    typeof row.user !== 'string' ||
    !row.user ||
    typeof row.epoch !== 'string' ||
    !row.epoch ||
    Object.keys(row).some((key) => key !== 'user' && key !== 'epoch')
  )
    throw storageFailure();
  return { user: row.user, epoch: row.epoch };
}

/** A speculative account read cannot replace an identity established while it waited. */
export function guardAgentTransportIdentity(storage: BrowserStorage): () => void {
  const identity = readIdentity(storage);
  return () => {
    const current = readIdentity(storage);
    if (current?.user !== identity?.user || current?.epoch !== identity?.epoch)
      throw new IdentityChangedError();
  };
}

/** Clear only this tab's Agent transport state after logout or an account change. */
export function clearAgentTransport(storage: BrowserStorage): void {
  const keys: string[] = [];
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (key?.startsWith(PREFIX)) keys.push(key);
  }
  // Removing identity first invalidates outstanding journal handles, even if a
  // subsequent storage deletion fails. An old asynchronous callback cannot write.
  storage.removeItem(IDENTITY);
  for (const key of keys) storage.removeItem(key);
}

/** Bind retained requests to one authenticated account and local sign-in epoch. */
export function agentTransportJournal(
  storage: BrowserStorage,
  user: string,
  agent: string,
  makeKey: () => string = () => crypto.randomUUID(),
): AgentTransportJournal {
  if (!user || !agent) throw new Error('Sign in and select an Agent before sending.');
  let identity = readIdentity(storage);
  if (identity?.user !== user) {
    clearAgentTransport(storage);
    identity = { user, epoch: makeKey() };
    storage.setItem(IDENTITY, JSON.stringify(identity));
  }
  return new AgentTransportJournal(storage, identity, agent, makeKey);
}

export function readRetainedPromptIntent(storage: BrowserStorage, user: string, agent: string) {
  const identity = readIdentity(storage);
  if (!user || !agent || identity?.user !== user) return undefined;
  return new AgentTransportJournal(storage, identity, agent, () => crypto.randomUUID())
    .promptSettingsIntent()
    .read();
}

export class AgentTransportJournal {
  private readonly key: string;
  private invalidated = false;
  readonly queuedCancellation: QueuedCancellationJournal;
  readonly queuedInputs: QueuedInputJournal;
  readonly interruption: InterruptionJournal;
  readonly legacySuggestion: LegacySuggestionJournal;

  constructor(
    private readonly storage: BrowserStorage,
    private readonly identity: { readonly user: string; readonly epoch: string },
    private readonly agent: string,
    private readonly makeKey: () => string,
  ) {
    this.key = `${PREFIX}${encodeURIComponent(identity.user)}:${encodeURIComponent(agent)}`;
    this.legacySuggestion = new LegacySuggestionJournal(
      storage,
      `${this.key}:suggestion:${identity.epoch}`,
      () => {
        if (this.current()?.transport !== 'agui')
          throw new Error('This Agent has not selected AG-UI.');
      },
      makeKey,
    );
    this.interruption = new InterruptionJournal(
      storage,
      `${this.key}:interruption:${identity.epoch}`,
      () => {
        if (this.current()?.transport !== 'responses_v2')
          throw new Error('This Agent has not selected the Responses transport.');
      },
      (input, key) => bindDraftCommand(identity.user, agent, input, key, storage),
      (key) => acceptDraftCommand(identity.user, agent, key, storage),
    );
    this.queuedInputs = new QueuedInputJournal(
      storage,
      `${this.key}:queued-inputs:${identity.epoch}`,
      () => {
        if (this.current()?.transport !== 'responses_v2')
          throw new Error('This Agent has not selected the Responses transport.');
      },
    );
    this.queuedCancellation = new QueuedCancellationJournal(
      storage,
      `${this.key}:queued-cancellation:v1`,
      agent,
      identity.epoch,
      () => {
        if (this.current()?.transport !== 'responses_v2')
          throw new Error('This Agent has not selected the Responses transport.');
      },
    );
  }

  get agentId(): string {
    return this.agent;
  }

  private requireIdentity(): void {
    if (this.invalidated) throw new IdentityChangedError();
    const identity = readIdentity(this.storage);
    if (identity?.user !== this.identity.user || identity.epoch !== this.identity.epoch)
      throw new IdentityChangedError();
  }

  settingsIntent<Body>(
    kind: RetainedSettingsEditor,
    parseBody: (value: unknown) => Body | undefined,
  ) {
    return settingsIntentJournal(
      this.storage,
      `${this.key}:configuration:${kind}:${this.identity.epoch}`,
      () => this.requireIdentity(),
      parseBody,
    );
  }

  promptSettingsIntent() {
    return promptIntentJournal(
      this.storage,
      `${this.key}:configuration:prompt-fingerprint:${this.identity.epoch}`,
      () => this.requireIdentity(),
    );
  }

  private current(): TransportRecord | undefined {
    this.requireIdentity();
    const source = this.storage.getItem(this.key);
    return source === null ? undefined : parseRecord(source, this.agent, this.identity.epoch);
  }

  /** An old request must never erase a newer account's journal. */
  invalidate(): void {
    this.current();
    this.invalidated = true;
    try {
      try {
        clearOwnerDrafts(this.identity.user, this.storage);
      } finally {
        clearAgentTransport(this.storage);
      }
    } catch (cause) {
      throw new IdentityChangedError({ cause });
    }
  }

  private save(value: TransportRecord): void {
    this.current();
    // Validate before writing, and save before any caller may perform network IO.
    const bytes = JSON.stringify(value);
    parseRecord(bytes, this.agent, this.identity.epoch);
    this.storage.setItem(this.key, bytes);
  }

  /** Selection is stable across reloads, flag changes and capability outages. */
  select(enabled: boolean): AgentTransport {
    const current = this.current();
    if (current !== undefined) return current.transport;
    const transport = enabled ? 'responses_v2' : 'agui';
    this.save({
      agent: this.agent,
      epoch: this.identity.epoch,
      transport,
      input: null,
      control: null,
    });
    return transport;
  }

  selected(): AgentTransport | undefined {
    return this.current()?.transport;
  }

  read(): InputRecord | null {
    return this.current()?.input ?? null;
  }

  /** Finish draft cleanup if a reload interrupted a previously confirmed receipt. */
  reconcileDraft(): void {
    try {
      const input = this.read();
      const control = this.readControl();
      const interruption = this.interruption.read();
      const accepted = [
        input?.state === 'accepted' ? input.key : undefined,
        control?.commandId != null ? control.key : undefined,
        interruption?.receipt != null ? interruption.key : undefined,
      ];
      for (const key of accepted)
        if (key) acceptDraftCommand(this.identity.user, this.agent, key, this.storage);
    } catch {
      /* Preserve drafts while the journal or identity cannot be read. */
    }
  }

  readControl(): ControlRecord | null {
    return this.current()?.control ?? null;
  }

  retainControl(control: ControlRecord): ControlRecord {
    if (this.interruption.read()?.receipt === null)
      throw new Error('Recover your pending interruption before requesting another action.');
    const current = this.current();
    if (current?.transport !== 'responses_v2')
      throw new Error('This Agent has not selected the Responses transport.');
    if (current.control?.commandId === null) {
      if (
        current.control.responseId !== control.responseId ||
        JSON.stringify(current.control.body) !== JSON.stringify(control.body)
      )
        throw new Error('Recover your pending action before requesting a different action.');
      return current.control;
    }
    this.save({ ...current, control });
    if (control.body.command.type === 'steer')
      bindDraftCommand(
        this.identity.user,
        this.agent,
        control.body.command.text,
        control.key,
        this.storage,
      );
    return this.readControl()!;
  }

  acceptedControl(key: string, commandId: string): void {
    const current = this.current();
    const control = current?.control;
    if (
      current === undefined ||
      control === null ||
      control === undefined ||
      control.key !== key ||
      (control.commandId !== null && control.commandId !== commandId)
    )
      throw new Error('The action receipt does not match the retained request.');
    this.save({ ...current, control: { ...control, commandId } });
    acceptDraftCommand(this.identity.user, this.agent, key, this.storage);
  }

  /** Only an authoritative refusal of this exact pending intent permits replacement. */
  refusedControl(key: string): void {
    const current = this.current();
    if (current?.control?.key !== key || current.control.commandId !== null)
      throw new Error('The refusal does not match the pending action.');
    this.save({ ...current, control: null });
  }

  /** Reuse the pending exact intent. A different message cannot replace uncertainty. */
  retain(input: string, answerContext?: AnswerContext): InputRecord {
    if (this.interruption.read()?.receipt === null)
      throw new Error('Recover your pending interruption before sending another input.');
    const current = this.current();
    if (current?.transport !== 'responses_v2')
      throw new Error('This Agent has not selected the Responses transport.');
    const previous = current.input;
    if (previous?.state === 'pending') {
      if (
        previous.input !== input ||
        JSON.stringify(previous.answerContext) !== JSON.stringify(answerContext)
      )
        throw new Error('Recover your pending message before sending a different message.');
      return previous;
    }
    const intent: InputRecord = {
      state: 'pending',
      key: this.makeKey(),
      input,
      receipt: null,
      ...(answerContext === undefined ? {} : { answerContext }),
    };
    if (
      bindDraftCommand(this.identity.user, this.agent, input, intent.key, this.storage) ===
      'unavailable'
    )
      throw new DraftBindingError();
    this.save({ ...current, input: intent });
    return intent;
  }

  /** Durable receipt retention must succeed before the composer clears its draft. */
  accepted(
    key: string,
    receipt: { readonly responseId: string; readonly commandId: string },
  ): void {
    const current = this.current();
    const input = current?.input;
    if (current === undefined || input === null || input === undefined || input.key !== key)
      throw new Error('The response receipt does not belong to the retained message.');
    if (
      input.receipt !== null &&
      (input.receipt.responseId !== receipt.responseId ||
        input.receipt.commandId !== receipt.commandId)
    )
      throw new Error('The recovered response receipt changed identity.');
    this.save({ ...current, input: { ...input, state: 'accepted', receipt: { ...receipt } } });
    acceptDraftCommand(this.identity.user, this.agent, key, this.storage);
  }

  /** Only an authoritative admission refusal permits replacing the pending intent. */
  refused(key: string): void {
    const current = this.current();
    if (current?.input?.key !== key || current.input.state !== 'pending')
      throw new Error('The refusal does not belong to the pending message.');
    this.save({ ...current, input: null });
  }
}
