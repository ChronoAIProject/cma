import { rateLimitDelay } from './readRetry';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { AgentV2Session } from './client';
import type { QueuedInputIntent } from './queuedInputJournal';
import { sendQueuedInputIntent } from './queuedInputs';
import { recoverQueuedCancellation } from './queuedCancellation';
import { observeRead } from './readObserver';
import { QueuedCommandTracker } from './queuedCommandTracker';
import type { QueuedInputOutcome } from './queuedCommandOutcomes';
import type { ConversationMessage } from '../model';

export interface QueuedInputRow {
  readonly responseId: string;
  readonly commandId: string;
  readonly revision: number;
  readonly input: string;
  readonly busy?: boolean;
  readonly error?: string;
  readonly needsRecovery?: boolean;
  readonly pendingEdit?: string;
}
export interface QueuedInputsControl {
  readonly items: readonly QueuedInputRow[];
  readonly outcomes: readonly QueuedInputOutcome[];
  readonly managedCommandIds: ReadonlySet<string>;
  readonly error?: string;
  readonly edit: (item: QueuedInputRow, input: string) => Promise<void>;
  readonly remove: (item: QueuedInputRow) => Promise<void>;
  readonly recover: (item: QueuedInputRow) => Promise<void>;
  readonly unappliedEdits?: readonly { key: string; text: string }[];
  readonly dismissUnappliedEdit?: (key: string) => void;
  readonly refresh: () => void;
}

export function useQueuedInputs(
  session: AgentV2Session | undefined,
  enabled: boolean,
  activeTurn: boolean,
  eventRevision: string,
  onIdentityChange: (error: unknown) => boolean,
  requestRevision = 0,
  messages?: readonly ConversationMessage[],
): QueuedInputsControl {
  const tracker = useMemo(
    () => (session && enabled ? new QueuedCommandTracker(session) : undefined),
    [session, enabled],
  );
  const [stored, setStored] = useState<{
    tracker: QueuedCommandTracker;
    view: ReturnType<QueuedCommandTracker['snapshot']>;
  }>();
  const [operations, setOperations] = useState<{
    tracker?: QueuedCommandTracker;
    busy: ReadonlySet<string>;
    errors: ReadonlyMap<string, string>;
  }>({ tracker, busy: new Set(), errors: new Map() });
  const actions = useRef(new Map<string, AbortController>());
  const errors = useRef(new Map<string, string>());
  const active = useRef(activeTurn);
  const scope = useRef({ session, enabled, messages });
  useLayoutEffect(() => {
    active.current = activeTurn;
    scope.current = { session, enabled, messages };
  }, [session, enabled, activeTurn, messages]);
  const schedule = useRef<ReturnType<typeof observeRead> | undefined>(undefined);
  const refresh = useCallback(() => {
    tracker?.refresh();
    schedule.current?.refresh();
  }, [tracker]);
  const observed = useRef({ session, eventRevision, activeTurn, requestRevision });
  const current = stored?.tracker === tracker ? stored?.view : undefined;
  const bump = useCallback(
    () =>
      setOperations({
        tracker,
        busy: new Set(actions.current.keys()),
        errors: new Map(errors.current),
      }),
    [tracker],
  );
  const local = operations.tracker === tracker ? operations : undefined;
  const publish = useCallback(() => {
    if (tracker) setStored({ tracker, view: tracker.snapshot() });
  }, [tracker]);

  useEffect(() => {
    errors.current.clear();
    bump();
    const pending = actions.current;
    return () => {
      for (const request of pending.values()) request.abort();
      pending.clear();
    };
  }, [session, enabled, bump]);

  useEffect(() => {
    if (!tracker || !enabled) return;
    let failures = 0;
    const observer = observeRead(async (signal) => {
      let changed = false;
      const identityChanged = (error: unknown) => {
        changed = changed || onIdentityChange(error);
        if (changed) setStored(undefined);
        return changed;
      };
      const current = () =>
        !signal.aborted && scope.current.enabled && scope.current.session === session;
      try {
        const result = await tracker.observe(
          signal,
          active.current,
          identityChanged,
          (commandId, responseId) =>
            scope.current.messages?.some(
              (message) =>
                message.role === 'user' &&
                message.status === 'complete' &&
                !message.liveOnly &&
                message.commandId === commandId &&
                message.responseId === responseId,
            ) ?? false,
          (responseId) => {
            if (errors.current.delete(responseId)) bump();
          },
          current,
        );
        if (!current() || changed) return;
        publish();
        failures = 0;
        return result;
      } catch (error) {
        if (!current() || changed || identityChanged(error)) return;
        const next = rateLimitDelay(error, Math.min(2000 * 2 ** ++failures, 10000), failures);
        setStored({
          tracker,
          view: {
            ...tracker.snapshot(),
            error: 'The message queue could not be refreshed. Your messages are retained.',
          },
        });
        return { next, cooldown: next, failed: true };
      }
    });
    schedule.current = observer;
    return () => {
      observer.dispose();
      if (schedule.current === observer) schedule.current = undefined;
    };
  }, [tracker, session, enabled, onIdentityChange, publish, bump]);

  useEffect(() => {
    const previous = observed.current;
    observed.current = { session, eventRevision, activeTurn, requestRevision };
    if (previous.session !== session) return;
    if (previous.requestRevision !== requestRevision || previous.activeTurn !== activeTurn)
      schedule.current?.refresh();
    else if (previous.eventRevision !== eventRevision) schedule.current?.hint();
  }, [session, eventRevision, activeTurn, requestRevision]);

  let intents: readonly QueuedInputIntent[] = [];
  let unappliedEdits: readonly QueuedInputIntent[] = [];
  let storageError: string | undefined;
  try {
    intents = session?.journal.queuedInputs.read() ?? [];
    unappliedEdits = session?.journal.queuedInputs.unappliedEdits() ?? [];
    session?.journal.queuedInputs.managedCommands();
    const legacy = session?.journal.queuedCancellation.read();
    if (legacy?.commandId === null) {
      const input = session?.journal.read();
      intents = [
        ...intents,
        {
          key: legacy.key,
          responseId: legacy.responseId,
          commandId: legacy.body.source_command_id,
          revision: legacy.body.expected_command_revision,
          action: 'remove',
          original:
            input?.receipt?.commandId === legacy.body.source_command_id
              ? input.input
              : 'Queued message awaiting cancellation confirmation.',
        },
      ];
    }
  } catch {
    storageError = 'Queue recovery is unavailable. Refresh status.';
  }
  const items = [...(current?.items ?? [])];
  const unappliedByResponse = new Map(unappliedEdits.map((intent) => [intent.responseId, intent]));
  for (const intent of intents)
    if (!items.some((item) => item.responseId === intent.responseId))
      items.push({
        responseId: intent.responseId,
        commandId: intent.commandId,
        revision: intent.revision,
        input: intent.original,
      });

  const mutate = async (
    item: QueuedInputRow,
    kind: 'edit' | 'remove' | 'recover',
    text?: string,
  ) => {
    if (!session || !tracker || !enabled || actions.current.has(item.responseId) || storageError)
      throw new Error('This queued message cannot be changed right now.');
    const request = new AbortController();
    actions.current.set(item.responseId, request);
    tracker.beginMutation(item);
    errors.current.delete(item.responseId);
    bump();
    try {
      await session.authorize(request.signal);
      session.journal.queuedInputs.remember([item.commandId]);
      const legacy = session.journal.queuedCancellation.read();
      if (
        kind === 'recover' &&
        legacy?.commandId === null &&
        legacy.responseId === item.responseId
      ) {
        const result = await recoverQueuedCancellation(session, request.signal);
        request.signal.throwIfAborted();
        tracker.finishMutation(item, result ? { response: result } : undefined);
        publish();
        return;
      }
      const previous = session.journal.queuedInputs
        .read()
        .find((entry) => entry.responseId === item.responseId);
      if (kind !== 'recover' && previous)
        throw new Error('Confirm the pending request for this message first.');
      const intent =
        kind === 'recover'
          ? previous
          : session.journal.queuedInputs.retain({
              key: crypto.randomUUID(),
              responseId: item.responseId,
              commandId: item.commandId,
              revision: item.revision,
              original: item.input,
              action: kind,
              ...(kind === 'edit' ? { text } : {}),
            });
      if (!intent) throw new Error('There is no pending request for this message.');
      const result = await sendQueuedInputIntent(session, intent, request.signal);
      request.signal.throwIfAborted();
      tracker.finishMutation(item, result);
      publish();
    } catch (error) {
      if (!request.signal.aborted) tracker.finishMutation(item);
      if (!request.signal.aborted && !onIdentityChange(error))
        errors.current.set(
          item.responseId,
          error instanceof Error ? error.message : 'The queued request could not be confirmed.',
        );
      throw error;
    } finally {
      if (actions.current.get(item.responseId) === request) {
        actions.current.delete(item.responseId);
        bump();
        refresh();
      }
    }
  };
  return {
    managedCommandIds: enabled ? (current?.managedCommandIds ?? new Set()) : new Set(),
    outcomes: enabled ? (current?.outcomes ?? []) : [],
    items: enabled
      ? items.map((item) => {
          const stale = unappliedByResponse.get(item.responseId);
          return {
            ...item,
            busy:
              item.busy ||
              local?.busy.has(item.responseId) ||
              (stale?.commandId === item.commandId && item.revision <= stale.revision),
            needsRecovery: intents.some((intent) => intent.responseId === item.responseId),
            pendingEdit: storageError
              ? undefined
              : intents.find(
                  (intent) =>
                    intent.action === 'edit' &&
                    intent.responseId === item.responseId &&
                    intent.commandId === item.commandId,
                )?.text,
            error: local?.errors.get(item.responseId),
          };
        })
      : [],
    unappliedEdits:
      enabled && !storageError
        ? unappliedEdits.flatMap(({ key, text }) =>
            text !== undefined && !intents.some((intent) => intent.key === key)
              ? [{ key, text }]
              : [],
          )
        : [],
    dismissUnappliedEdit: (key) => {
      if (!enabled || !session) return;
      session.journal.queuedInputs.dismissUnappliedEdit(key);
      bump();
    },
    error: enabled ? (storageError ?? current?.error) : undefined,
    edit: (item, input) => mutate(item, 'edit', input),
    remove: (item) => mutate(item, 'remove'),
    recover: (item) => mutate(item, 'recover'),
    refresh,
  };
}
