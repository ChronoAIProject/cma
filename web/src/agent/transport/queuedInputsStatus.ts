import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { AgentV2Session } from './client';
import type { QueuedInputIntent } from './queuedInputJournal';
import { sendQueuedInputIntent } from './queuedInputs';
import { recoverQueuedCancellation } from './queuedCancellation';

export interface QueuedInputRow {
  readonly responseId: string;
  readonly commandId: string;
  readonly revision: number;
  readonly input: string;
  readonly busy?: boolean;
  readonly error?: string;
  readonly needsRecovery?: boolean;
}
export interface QueuedInputsControl {
  readonly items: readonly QueuedInputRow[];
  readonly managedCommandIds: ReadonlySet<string>;
  readonly error?: string;
  readonly edit: (item: QueuedInputRow, input: string) => Promise<void>;
  readonly remove: (item: QueuedInputRow) => Promise<void>;
  readonly recover: (item: QueuedInputRow) => Promise<void>;
  readonly refresh: () => void;
}

export function useQueuedInputs(
  session: AgentV2Session | undefined,
  enabled: boolean,
  activeTurn: boolean,
  eventRevision: string,
  onIdentityChange: (error: unknown) => boolean,
): QueuedInputsControl {
  const [stored, setStored] = useState<{
    session: AgentV2Session;
    items: readonly QueuedInputRow[];
    error?: string;
  }>();
  const [operations, setOperations] = useState<{
    session?: AgentV2Session;
    busy: ReadonlySet<string>;
    errors: ReadonlyMap<string, string>;
    managed: ReadonlySet<string>;
  }>({ session, busy: new Set(), errors: new Map(), managed: new Set() });
  const actions = useRef(new Map<string, AbortController>());
  const errors = useRef(new Map<string, string>());
  const generation = useRef(0);
  const managed = useRef(new Set<string>());
  const active = useRef(activeTurn);
  useLayoutEffect(() => {
    active.current = activeTurn;
  }, [activeTurn]);
  const schedule = useRef<(() => void) | undefined>(undefined);
  const refresh = useCallback(() => schedule.current?.(), []);
  const current = stored?.session === session ? stored : undefined;
  const bump = useCallback(
    () =>
      setOperations({
        session,
        busy: new Set(actions.current.keys()),
        errors: new Map(errors.current),
        managed: new Set(managed.current),
      }),
    [session],
  );
  const local = operations.session === session ? operations : undefined;

  useEffect(() => {
    errors.current.clear();
    managed.current.clear();
    if (session) {
      try {
        for (const id of session.journal.queuedInputs.managedCommands()) managed.current.add(id);
      } catch {
        /* The ordinary read below reports storage recovery errors. */
      }
    }
    bump();
    const pending = actions.current;
    return () => {
      for (const request of pending.values()) request.abort();
      pending.clear();
    };
  }, [session, enabled, bump]);

  useEffect(() => {
    if (!session || !enabled) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let reading = false;
    let again = false;
    let retry = 1000;
    const read = async () => {
      if (request.signal.aborted || document.visibilityState === 'hidden') return;
      if (reading) {
        again = true;
        return;
      }
      reading = true;
      const observedGeneration = generation.current;
      clearTimeout(timer);
      let follow = false;
      try {
        await session.authorize(request.signal);
        const page = await session.responses.listQueuedInputs(session.journal.agentId, {
          signal: request.signal,
        });
        await session.authorize(request.signal);
        if (observedGeneration !== generation.current) {
          again = true;
          return;
        }
        const items = page.data.map((item): QueuedInputRow => ({
          responseId: item.response.receipt.response_id!,
          commandId: item.response.receipt.command_id,
          revision: item.command_revision,
          input: item.text,
          busy: !item.editable,
        }));
        for (const item of items) managed.current.add(item.commandId);
        session.journal.queuedInputs.remember(items.map((item) => item.commandId));
        const last = session.journal.read();
        if (last?.state === 'accepted' && !managed.current.has(last.receipt.commandId)) {
          try {
            const status = await session.responses.readCommand(last.receipt.commandId, {
              signal: request.signal,
            });
            await session.authorize(request.signal);
            if (
              status.receipt.agent_id === session.journal.agentId &&
              status.receipt.response_id === last.receipt.responseId &&
              (status.delivery !== null ||
                ['completed', 'cancelled', 'failed'].includes(status.status))
            ) {
              managed.current.add(last.receipt.commandId);
              session.journal.queuedInputs.remember([last.receipt.commandId]);
            }
          } catch (error) {
            if (request.signal.aborted || onIdentityChange(error)) return;
          }
        }
        if (observedGeneration !== generation.current) {
          again = true;
          return;
        }
        setStored({ session, items });
        bump();
        retry = 2000;
        follow = items.length > 0 || active.current;
      } catch (error) {
        if (request.signal.aborted || onIdentityChange(error)) return;
        // Retained older deployments do not have editable queue discovery.
        if (error instanceof ApiError && error.status === 404) return;
        setStored((previous) => ({
          session,
          items: previous?.session === session ? previous.items : [],
          error: 'The message queue could not be refreshed. Your messages are retained.',
        }));
        follow = true;
        retry = Math.min(retry * 2, 10000);
      } finally {
        reading = false;
        if (!request.signal.aborted) {
          if (again) {
            again = false;
            timer = setTimeout(() => void read(), 100);
          } else if (follow) timer = setTimeout(() => void read(), retry);
        }
      }
    };
    const invalidate = () => {
      clearTimeout(timer);
      if (reading) again = true;
      else timer = setTimeout(() => void read(), 100);
    };
    schedule.current = invalidate;
    document.addEventListener('visibilitychange', invalidate);
    window.addEventListener('focus', invalidate);
    void read();
    return () => {
      request.abort();
      clearTimeout(timer);
      if (schedule.current === invalidate) schedule.current = undefined;
      document.removeEventListener('visibilitychange', invalidate);
      window.removeEventListener('focus', invalidate);
    };
  }, [session, enabled, onIdentityChange, bump]);

  useEffect(refresh, [refresh, eventRevision, activeTurn]);

  let intents: readonly QueuedInputIntent[] = [];
  let storageError: string | undefined;
  try {
    intents = session?.journal.queuedInputs.read() ?? [];
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
  } catch (error) {
    storageError = error instanceof Error ? error.message : 'Queue recovery is unavailable.';
  }
  const items = [...(current?.items ?? [])];
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
    if (!session || !enabled || actions.current.has(item.responseId) || storageError)
      throw new Error('This queued message cannot be changed right now.');
    const request = new AbortController();
    actions.current.set(item.responseId, request);
    generation.current += 1;
    managed.current.add(item.commandId);
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
        await recoverQueuedCancellation(session, request.signal);
        generation.current += 1;
        setStored((state) =>
          state?.session === session
            ? {
                ...state,
                items: state.items.filter((entry) => entry.responseId !== item.responseId),
              }
            : state,
        );
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
      await sendQueuedInputIntent(session, intent, request.signal);
      generation.current += 1;
      setStored((state) =>
        state?.session !== session
          ? state
          : {
              ...state,
              items:
                intent.action === 'remove'
                  ? state.items.filter((entry) => entry.responseId !== item.responseId)
                  : state.items.map((entry) =>
                      entry.responseId === item.responseId
                        ? { ...entry, input: intent.text!, busy: true }
                        : entry,
                    ),
            },
      );
    } catch (error) {
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
    managedCommandIds: local?.managed ?? new Set(),
    items: enabled
      ? items.map((item) => ({
          ...item,
          busy: item.busy || local?.busy.has(item.responseId),
          needsRecovery: intents.some((intent) => intent.responseId === item.responseId),
          error: local?.errors.get(item.responseId),
        }))
      : [],
    error: storageError ?? current?.error,
    edit: (item, input) => mutate(item, 'edit', input),
    remove: (item) => mutate(item, 'remove'),
    recover: (item) => mutate(item, 'recover'),
    refresh,
  };
}
