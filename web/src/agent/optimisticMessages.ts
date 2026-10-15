import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { MessageAcceptance } from './controllerTypes';
import type { ConversationState } from './conversation';
import type { AgentV2Session } from './transport/client';

export interface OptimisticMessage {
  readonly id: string;
  readonly content: string;
  readonly state: 'sending' | 'accepted' | 'unconfirmed' | 'not_sent';
  readonly receipt?: MessageAcceptance;
  readonly key?: string;
  readonly queued?: boolean;
  readonly source?: 'steer';
}

/** Local presentation only: it grants no authority and never submits or retries an input. */
export function useOptimisticMessages(
  identity: object,
  enabled: boolean,
  conversation: ConversationState | undefined,
  session: AgentV2Session | undefined,
) {
  const scope = useMemo(() => ({ identity, enabled }), [identity, enabled]);
  const active = useRef<object | undefined>(scope);
  const [stored, setStored] = useState<{ scope: object; rows: readonly OptimisticMessage[] }>({
    scope,
    rows: [],
  });
  useLayoutEffect(() => {
    active.current = scope;
    return () => {
      active.current = undefined;
    };
  }, [scope]);
  const rows = stored.scope === scope && enabled ? stored.rows : [];
  const update = (change: (rows: readonly OptimisticMessage[]) => readonly OptimisticMessage[]) => {
    if (!enabled || active.current !== scope) return;
    setStored((current) => ({ scope, rows: change(current.scope === scope ? current.rows : []) }));
  };
  const retained = (mode?: 'queue' | 'steer') => {
    try {
      if (mode === 'steer') {
        // A temporarily unavailable session is different from a successful
        // recovery that explicitly removed the control intent. Preserve the
        // local row while the session is reconnecting so it cannot be shown
        // as refused merely because the journal is inaccessible.
        if (!session) return undefined;
        const control = session.journal.readControl();
        if (!control || control.body.command.type !== 'steer') return null;
        return {
          key: control.key,
          input: control.body.command.text,
          state: control.commandId === null ? ('pending' as const) : ('accepted' as const),
          receipt:
            control.commandId === null
              ? undefined
              : { commandId: control.commandId, responseId: control.responseId },
        };
      }
      return session?.journal.read();
    } catch {
      return undefined;
    }
  };
  const begin = (content: string, mode?: 'queue' | 'steer') => {
    const id = crypto.randomUUID();
    const previous = retained(mode);
    const source = mode === 'steer' ? ('steer' as const) : undefined;
    const previousKey = previous?.key;
    update((current) => [
      // A different next draft must not discard a refused message's text/files.
      ...current.filter((row) => row.state !== 'not_sent' || row.content !== content),
      ...(previous?.state === 'accepted' && !current.some((row) => row.key === previous.key)
        ? [
            {
              id: previous.key,
              key: previous.key,
              content: previous.input,
              state: 'accepted' as const,
              receipt: previous.receipt,
              source,
            },
          ]
        : []),
      { id, content, state: 'sending', queued: mode === 'queue', source },
    ]);
    return {
      observed(receipt: MessageAcceptance) {
        const input = retained(mode);
        update((current) =>
          current.map((row) => (row.id === id ? { ...row, receipt, key: input?.key } : row)),
        );
      },
      accepted(receipt?: MessageAcceptance) {
        if (!enabled || active.current !== scope) return false;
        const input = retained(mode);
        update((current) =>
          current.map((row) =>
            row.id === id ? { ...row, state: 'accepted', receipt, key: input?.key } : row,
          ),
        );
        return true;
      },
      settled() {
        const input = retained(mode);
        update((current) =>
          current.map((row) =>
            row.id !== id || row.state === 'accepted' || row.state === 'not_sent'
              ? row
              : input?.input === content && input.key !== previousKey
                ? {
                    ...row,
                    key: input.key,
                    state: input.state === 'pending' ? 'unconfirmed' : 'accepted',
                    receipt: input.receipt ?? row.receipt,
                  }
                : { ...row, state: 'unconfirmed' },
          ),
        );
      },
      cancelled() {
        update((current) =>
          current.map((row) => (row.id === id ? { ...row, state: 'not_sent' } : row)),
        );
      },
    };
  };
  const input = enabled ? retained() : undefined;
  const steering = enabled ? retained('steer') : undefined;
  // The journal survives reload; its exact key owns the recoverable pending presentation.
  const pending = [
    { record: input, source: undefined },
    { record: steering, source: 'steer' as const },
  ].flatMap(({ record, source }) =>
    record !== null &&
    record !== undefined &&
    !rows.some(
      (row) => row.source === source && (row.key === record.key || row.state === 'sending'),
    )
      ? [
          {
            id: record.key,
            key: record.key,
            content: record.input,
            state: record.state === 'pending' ? ('unconfirmed' as const) : ('accepted' as const),
            receipt: record.receipt ?? undefined,
            source,
          },
        ]
      : [],
  );
  const visible = [...rows, ...pending]
    .map((row): OptimisticMessage => {
      const record = row.source === 'steer' ? steering : input;
      if (row.key !== undefined && record?.key === row.key && record.state === 'accepted')
        return { ...row, state: 'accepted', receipt: record.receipt };
      // A pending control can disappear only after a definitive no-admission response.
      if (
        row.source === 'steer' &&
        row.key !== undefined &&
        row.state === 'unconfirmed' &&
        steering === null
      )
        return { ...row, state: 'not_sent' };
      return row;
    })
    .filter(
      (row) =>
        !conversation?.messages.some(
          (message) =>
            message.role === 'user' &&
            row.receipt !== undefined &&
            ('commandId' in row.receipt
              ? message.commandId === row.receipt.commandId
              : message.runId === row.receipt.runId),
        ),
    );
  return { begin, messages: visible };
}
