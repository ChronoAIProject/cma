import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { MessageAcceptance } from './controllerTypes';
import type { ConversationState } from './conversation';
import type { AgentV2Session } from './transport/client';
import type { AgentController } from './controllerTypes';

export interface OptimisticMessage {
  readonly id: string;
  readonly content: string;
  readonly at?: string;
  readonly state: 'sending' | 'accepted' | 'unconfirmed' | 'not_sent';
  readonly receipt?: MessageAcceptance;
  readonly key?: string;
  readonly queued?: boolean;
  readonly queueBeforeSend?: readonly string[];
  readonly source?: 'steer' | 'interrupt';
}

/** Local presentation only: it grants no authority and never submits or retries an input. */
export function useOptimisticMessages(
  identity: object,
  enabled: boolean,
  conversation: ConversationState | undefined,
  session: AgentV2Session | undefined,
  readSuggestionRequest?: AgentController['readSuggestionRequest'],
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
  const retained = (mode?: 'queue' | 'steer' | 'interrupt') => {
    try {
      if (mode === 'interrupt') {
        if (!session) return undefined;
        const intent = session.journal.interruption.read();
        if (!intent) return null;
        return {
          key: intent.key,
          input: intent.body.text,
          state: intent.receipt === null ? ('pending' as const) : ('accepted' as const),
          receipt: intent.receipt ?? undefined,
        };
      }
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
      return session ? session.journal.read() : readSuggestionRequest?.();
    } catch {
      return undefined;
    }
  };
  const begin = (
    content: string,
    mode?: 'queue' | 'steer' | 'interrupt',
    queueBeforeSend?: readonly string[],
  ) => {
    const id = crypto.randomUUID();
    const previous = retained(mode);
    const source = mode === 'steer' || mode === 'interrupt' ? mode : undefined;
    let previousKey = previous?.key;
    const ordinaryInput = mode === undefined || mode === 'queue' ? previous : retained();
    const ordinaryInputReleased = session !== undefined && ordinaryInput === null;
    update((current) => [
      // A different next draft must not discard a refused message's text/files.
      ...current
        .map((row) => ordinaryRefused(row, ordinaryInputReleased))
        .filter((row) => row.state !== 'not_sent' || row.content !== content),
      ...(previous?.state === 'accepted' && !current.some((row) => row.key === previous.key)
        ? [
            {
              id: previous.key,
              key: previous.key,
              content: previous.input,
              state: 'accepted' as const,
              receipt: previous.receipt ?? undefined,
              source,
            },
          ]
        : []),
      {
        id,
        content,
        at: new Date().toISOString(),
        state: 'sending',
        queued: mode === 'queue',
        source,
        queueBeforeSend,
      },
    ]);
    return {
      ordinaryInput() {
        // Only a conclusive inactive-target read can switch before submission.
        // Future receipt/recovery callbacks must now read the ordinary journal.
        mode = undefined;
        previousKey = retained()?.key;
        update((current) =>
          current.map((row) => (row.id === id ? { ...row, source: undefined } : row)),
        );
      },
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
                : // A readable empty journal proves no request was retained for
                  // dispatch. Missing authority or an observed receipt does not.
                  input === null && row.receipt === undefined
                  ? { ...row, state: 'not_sent' }
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
  const interruption = enabled ? retained('interrupt') : undefined;
  // The journal survives reload; its exact key owns the recoverable pending presentation.
  const pending = [
    { record: input, source: undefined },
    { record: steering, source: 'steer' as const },
    { record: interruption, source: 'interrupt' as const },
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
      const record =
        row.source === 'steer' ? steering : row.source === 'interrupt' ? interruption : input;
      if (row.key !== undefined && record?.key === row.key && record.state === 'accepted')
        return { ...row, state: 'accepted', receipt: record.receipt ?? undefined };
      // A pending control can disappear only after a definitive no-admission response.
      if (
        (row.source === 'steer' || row.source === 'interrupt') &&
        row.key !== undefined &&
        row.state === 'unconfirmed' &&
        record === null
      )
        return { ...row, state: 'not_sent' };
      return ordinaryRefused(row, session !== undefined && input === null);
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

function ordinaryRefused(row: OptimisticMessage, inputReleased: boolean): OptimisticMessage {
  if (
    inputReleased &&
    row.source === undefined &&
    row.key !== undefined &&
    row.state === 'unconfirmed' &&
    row.receipt === undefined
  )
    return { ...row, state: 'not_sent' };
  return row;
}
