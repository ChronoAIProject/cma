import { ApiError, type V2CommandStatus } from '@cma/client';
import type { AgentV2Session } from './client';
import type { QueuedInputRow } from './queuedInputsStatus';
import {
  commandOutcome,
  queuedOutcomeText,
  type QueuedInputOutcome,
} from './queuedCommandOutcomes';
import { rateLimitDelay, readRetryDelay } from './readRetry';
import { pendingRemovals, type PendingRemoval } from './queuedRemoval';

interface KnownCommand {
  commandId: string;
  responseId?: string;
  input?: string;
  row?: QueuedInputRow;
  present: boolean;
  outcome: Pick<QueuedInputOutcome, 'kind' | 'terminal'>;
  revision: number;
  mutating: boolean;
  due: number;
  failures: number;
  delivered?: boolean;
}
const queueError = 'The message queue could not be refreshed. Your messages are retained.';

/** One lifetime owns command evidence; discovery absence only removes editability. */
export class QueuedCommandTracker {
  private readonly known = new Map<string, KnownCommand>();
  private generation = 0;
  private cursor = 0;
  private discoveryRetryAt = 0;
  private discoveryPollAt = 0;
  private refreshRevision = 0;
  private queueRevision?: string;
  private discoveryNext = Infinity;
  private discoveryFailures = 0;
  private discoveryRetry = 1000;
  private throttleUntil = 0;
  private discoveryError?: string;
  private storageError?: string;

  constructor(private readonly session: AgentV2Session) {}

  private retain(commandId: string, responseId?: string, input?: string) {
    let entry = this.known.get(commandId);
    if (!entry) {
      entry = {
        commandId,
        present: false,
        outcome: { kind: 'unknown', terminal: false },
        revision: 0,
        mutating: false,
        due: 0,
        failures: 0,
      };
      this.known.set(commandId, entry);
    }
    if (responseId && entry.responseId && responseId !== entry.responseId)
      throw new Error('Command response changed.');
    entry.responseId ??= responseId;
    entry.input ??= input;
    return entry;
  }

  private validate(entry: KnownCommand, status: V2CommandStatus) {
    if (
      status.receipt.command_id !== entry.commandId ||
      status.receipt.agent_id !== this.session.journal.agentId ||
      status.receipt.response_id === null ||
      (entry.responseId !== undefined && status.receipt.response_id !== entry.responseId)
    )
      throw new Error('Command identity did not match.');
    return status.receipt.response_id;
  }

  private seed() {
    let removals: PendingRemoval[] = [];
    for (const { receipt, input } of this.session.knownInputs())
      this.retain(receipt.commandId, receipt.responseId, input);
    try {
      for (const id of this.session.journal.queuedInputs.managedCommands()) this.retain(id);
      const last = this.session.journal.read();
      if (last?.state === 'accepted')
        this.retain(last.receipt.commandId, last.receipt.responseId, last.input);
      const interruption = this.session.journal.interruption.read();
      if (interruption?.receipt)
        this.retain(
          interruption.receipt.commandId,
          interruption.receipt.responseId,
          interruption.body.text,
        );
      const intents = this.session.journal.queuedInputs.read();
      for (const intent of intents)
        this.retain(intent.commandId, intent.responseId, intent.original);
      removals = pendingRemovals(this.session, intents);
      for (const intent of removals) this.retain(intent.commandId, intent.responseId, intent.input);
      this.storageError = undefined;
    } catch {
      this.storageError = 'Queued message recovery is unavailable. Your messages are retained.';
    }
    return removals;
  }

  private remember() {
    try {
      this.session.journal.queuedInputs.remember([...this.known.keys()]);
    } catch {
      this.storageError =
        'Queued message recovery could not be saved. Keep this page open and refresh status.';
    }
  }

  snapshot() {
    const items: QueuedInputRow[] = [];
    const outcomes: QueuedInputOutcome[] = [];
    let unresolvedIdentity = false;
    for (const entry of this.known.values()) {
      if (entry.delivered && !entry.outcome.terminal) continue;
      if (entry.present && entry.row && !entry.outcome.terminal) items.push(entry.row);
      else if (entry.responseId)
        outcomes.push({
          commandId: entry.commandId,
          responseId: entry.responseId,
          input: entry.input,
          ...entry.outcome,
        });
      else unresolvedIdentity = true;
      if (entry.outcome.kind === 'unknown') unresolvedIdentity = true;
    }
    return {
      items,
      outcomes,
      managedCommandIds: new Set(this.known.keys()),
      error:
        this.storageError ??
        this.discoveryError ??
        (unresolvedIdentity ? queuedOutcomeText.unknown : undefined),
    };
  }

  refresh(allowCompatibilityRetry = true) {
    this.refreshRevision += 1;
    this.discoveryPollAt = 0;
    if (allowCompatibilityRetry && this.discoveryFailures === 0) this.discoveryRetryAt = 0;
  }

  beginMutation(item: QueuedInputRow) {
    const entry = this.retain(item.commandId, item.responseId, item.input);
    entry.mutating = true;
    entry.revision += 1;
    this.generation += 1;
  }

  finishMutation(
    item: QueuedInputRow,
    result?: { response: V2CommandStatus; row?: QueuedInputRow },
  ) {
    const entry = this.retain(item.commandId, item.responseId, item.input);
    entry.mutating = false;
    entry.revision += 1;
    this.generation += 1;
    entry.due = 0;
    if (!result) return;
    this.validate(entry, result.response);
    entry.outcome = commandOutcome(result.response);
    entry.row = result.row;
    entry.present = result.row !== undefined && !entry.outcome.terminal;
    if (result.row) entry.input = result.row.input;
  }

  async observe(
    signal: AbortSignal,
    active: boolean,
    identityChanged: (error: unknown) => boolean,
    hasDelivered?: (commandId: string, responseId: string) => boolean,
    resolved?: (responseId: string) => void,
    isCurrent = () => !signal.aborted,
  ) {
    if (Date.now() < this.throttleUntil)
      return { next: Math.max(2000, this.throttleUntil - Date.now()), cooldown: 2000 };
    await this.session.authorize(signal);
    if (!isCurrent()) return;
    const removals = this.seed();
    for (const intent of removals) {
      const entry = this.known.get(intent.commandId);
      if (
        hasDelivered?.(intent.commandId, intent.responseId) &&
        entry?.responseId === intent.responseId &&
        !entry.mutating &&
        this.resolve(intent, resolved)
      ) {
        entry.delivered = true;
        entry.present = false;
        entry.row = undefined;
      }
    }
    const discoveryAfter = Math.max(this.discoveryRetryAt, this.discoveryPollAt);
    if (Date.now() >= discoveryAfter) {
      const generation = this.generation;
      const refreshRevision = this.refreshRevision;
      try {
        const page = await this.session.responses.listQueuedInputs(this.session.journal.agentId, {
          signal,
        });
        await this.session.authorize(signal);
        signal.throwIfAborted();
        if (generation === this.generation) {
          // Validate all identities before changing any queue membership.
          for (const item of page.data)
            this.validate(this.retain(item.response.receipt.command_id), item.response);
          const listed = new Set(page.data.map((item) => item.response.receipt.command_id));
          for (const entry of this.known.values()) {
            if (entry.present && !listed.has(entry.commandId)) entry.due = 0;
            entry.present = false;
          }
          for (const item of page.data) {
            const receipt = item.response.receipt;
            const entry = this.retain(receipt.command_id, receipt.response_id!, item.text);
            if (entry.delivered || entry.outcome.terminal || entry.mutating) continue;
            if (entry.row && entry.row.revision > item.command_revision) {
              entry.present = true;
              continue;
            }
            entry.row = {
              responseId: receipt.response_id!,
              commandId: receipt.command_id,
              revision: item.command_revision,
              input: item.text,
              busy: !item.editable,
            };
            entry.input = item.text;
            entry.present = true;
            entry.outcome = commandOutcome(item.response);
            if (!removals.some((intent) => intent.commandId === entry.commandId)) entry.due = 0;
          }
          this.discoveryError = undefined;
          this.discoveryFailures = 0;
          this.discoveryRetryAt = 0;
          this.discoveryRetry = 2000;
        }
        const revision = JSON.stringify(
          page.data.map(({ response, command_revision, editable }) => [
            response.receipt.command_id,
            command_revision,
            editable,
            response.status,
            response.delivery,
            response.failure,
          ]),
        );
        const unchanged = this.queueRevision === revision;
        this.queueRevision = revision;
        const idle = page.data.length === 0 && !active;
        const delay = page.data.length > 0 && !unchanged ? 2000 : 10000;
        this.discoveryNext = idle ? Infinity : Date.now() + delay;
        // An observer wake for another command must preserve discovery's cadence.
        if (refreshRevision === this.refreshRevision)
          this.discoveryPollAt = idle ? 0 : this.discoveryNext;
      } catch (error) {
        if (signal.aborted || identityChanged(error)) throw error;
        if (error instanceof ApiError && error.status === 404) {
          this.discoveryRetryAt = Date.now() + 2000;
          this.discoveryNext = Date.now() + 30000;
        } else {
          this.discoveryError = queueError;
          this.discoveryRetry = Math.min(this.discoveryRetry * 2, 10000);
          const delay = rateLimitDelay(error, this.discoveryRetry, ++this.discoveryFailures);
          this.discoveryNext = this.discoveryRetryAt = Date.now() + delay;
          this.throttle(error, delay);
        }
      }
    } else {
      // An early refresh must retain the discovery read deferred by its cooldown.
      this.discoveryNext = Math.min(this.discoveryNext, discoveryAfter);
    }
    this.remember();
    await this.reconcile(signal, identityChanged, removals, resolved, isCurrent);
    signal.throwIfAborted();
    return this.schedule(removals);
  }

  private throttle(error: unknown, delay: number) {
    if (error instanceof ApiError && (error.status === 429 || error.retryAfterMs !== undefined))
      this.throttleUntil = Math.max(this.throttleUntil, Date.now() + delay);
  }

  private resolve(intent: PendingRemoval, resolved?: (responseId: string) => void) {
    try {
      if (!intent.resolve()) return false;
      resolved?.(intent.responseId);
      return true;
    } catch {
      this.storageError = 'Queued message recovery is unavailable. Your messages are retained.';
      return false;
    }
  }

  private async reconcile(
    signal: AbortSignal,
    identityChanged: (error: unknown) => boolean,
    removals: readonly PendingRemoval[],
    resolved: ((responseId: string) => void) | undefined,
    isCurrent: () => boolean,
  ) {
    const entries = [...this.known.values()];
    const batch: KnownCommand[] = [];
    for (let scanned = 0; scanned < entries.length && batch.length < 8; scanned++) {
      const entry = entries[this.cursor % entries.length]!;
      this.cursor = (this.cursor + 1) % entries.length;
      if (this.needsRead(entry, removals) && entry.due <= Date.now()) batch.push(entry);
    }
    let index = 0;
    const worker = async () => {
      while (index < batch.length && Date.now() >= this.throttleUntil) {
        const entry = batch[index++]!;
        const revision = entry.revision;
        try {
          await this.session.authorize(signal);
          const status = await this.session.responses.readCommand(entry.commandId, { signal });
          await this.session.authorize(signal);
          if (!isCurrent()) return;
          if (entry.revision !== revision || entry.mutating) continue;
          entry.responseId = this.validate(entry, status);
          const outcome = commandOutcome(status);
          if (!entry.outcome.terminal || outcome.terminal) entry.outcome = outcome;
          if (outcome.terminal || status.delivery === 'execution_observed') {
            for (const intent of removals)
              if (
                intent.commandId === entry.commandId &&
                intent.responseId === entry.responseId &&
                this.resolve(intent, resolved)
              ) {
                entry.present = false;
                if (status.delivery === 'execution_observed') entry.delivered = true;
              }
          }
          entry.failures = 0;
          entry.due =
            entry.outcome.terminal &&
            !removals.some((intent) => intent.commandId === entry.commandId)
              ? Infinity
              : Date.now() + 10000;
        } catch (error) {
          if (signal.aborted || identityChanged(error)) throw error;
          if (entry.revision !== revision || entry.mutating) continue;
          if (!entry.outcome.terminal) entry.outcome = { kind: 'unknown', terminal: false };
          const delay = readRetryDelay(error, ++entry.failures, 2000);
          entry.due = Date.now() + delay;
          this.throttle(error, delay);
        }
      }
    };
    await Promise.all([worker(), worker()]);
  }

  private schedule(removals: readonly PendingRemoval[] = []) {
    let nextAt = this.discoveryNext;
    for (const entry of this.known.values())
      if (this.needsRead(entry, removals)) nextAt = Math.min(nextAt, entry.due);
    nextAt = Math.max(nextAt, this.throttleUntil);
    const next = Number.isFinite(nextAt) ? Math.max(2000, nextAt - Date.now()) : undefined;
    return { next, cooldown: Math.max(2000, this.discoveryPollAt - Date.now()) };
  }

  private needsRead(entry: KnownCommand, removals: readonly PendingRemoval[]) {
    return (
      !entry.mutating &&
      ((!entry.present && !entry.outcome.terminal) ||
        removals.some((intent) => intent.commandId === entry.commandId))
    );
  }
}
