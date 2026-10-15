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
  const retained = () => {
    try {
      return session?.journal.read();
    } catch {
      return undefined;
    }
  };
  const begin = (content: string) => {
    const id = crypto.randomUUID();
    const previous = retained();
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
            },
          ]
        : []),
      { id, content, state: 'sending' },
    ]);
    return {
      observed(receipt: MessageAcceptance) {
        const input = retained();
        update((current) =>
          current.map((row) => (row.id === id ? { ...row, receipt, key: input?.key } : row)),
        );
      },
      accepted(receipt?: MessageAcceptance) {
        if (!enabled || active.current !== scope) return false;
        const input = retained();
        update((current) =>
          current.map((row) =>
            row.id === id ? { ...row, state: 'accepted', receipt, key: input?.key } : row,
          ),
        );
        return true;
      },
      settled() {
        const input = retained();
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
  // The journal survives reload; its exact key owns the recoverable pending presentation.
  const pending =
    input !== null &&
    input !== undefined &&
    !rows.some((row) => row.key === input.key || row.state === 'sending')
      ? [
          {
            id: input.key,
            key: input.key,
            content: input.input,
            state: input.state === 'pending' ? ('unconfirmed' as const) : ('accepted' as const),
            receipt: input.receipt ?? undefined,
          },
        ]
      : [];
  const visible = [...rows, ...pending]
    .map((row): OptimisticMessage =>
      row.key !== undefined && input?.key === row.key && input.state === 'accepted'
        ? { ...row, state: 'accepted', receipt: input.receipt }
        : row,
    )
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
