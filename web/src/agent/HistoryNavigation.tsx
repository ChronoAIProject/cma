import { useEffect, useMemo, useRef, useState } from 'react';
import type { V2AgentHistoryWindow, V2AgentStatePage } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import { Modal } from '../ui/Modal';
import { Button } from '../ui/Button';
import { MessageStream } from './MessageStream';
import { conversationFromCurrentState, conversationFromWindow } from './historyWindow';
import type { ConversationState } from './model';
import type { ApprovalDecision } from './controllerTypes';

interface Props {
  readonly client?: CmaWebClient['agentHistory'];
  readonly agentId: string;
  readonly scope?: unknown;
  readonly conversation: ConversationState;
  readonly disabled?: boolean;
  readonly approvalReadOnly?: boolean;
  readonly approvalBusy?: boolean;
  readonly authorize?: (signal: AbortSignal) => Promise<unknown>;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
}

/** Only one older window is retained. Browsing never changes the live cursor. */
export function HistoryNavigation({
  client,
  agentId,
  scope,
  conversation,
  disabled = false,
  authorize,
  onApproval,
  approvalReadOnly,
  approvalBusy,
}: Props) {
  const basis = useMemo(
    () => ({ client, agentId, scope, disabled }),
    [client, agentId, scope, disabled],
  );
  const request = useRef<AbortController | undefined>(undefined);
  const [view, setView] = useState<{
    basis: object;
    mode: 'history' | 'approvals';
    loading?: boolean;
    error?: string;
    history?: V2AgentHistoryWindow;
    approvals?: V2AgentStatePage;
  }>();
  useEffect(() => () => request.current?.abort(), [basis]);
  const current = !disabled && view?.basis === basis ? view : undefined;
  const close = () => {
    request.current?.abort();
    setView(undefined);
  };
  const read = async (mode: 'history' | 'approvals', cursor?: string, through?: string) => {
    if (client === undefined || disabled) return;
    request.current?.abort();
    const pending = new AbortController();
    request.current = pending;
    setView({ basis, mode, loading: true });
    try {
      await authorize?.(pending.signal);
      pending.signal.throwIfAborted();
      const result =
        mode === 'history'
          ? {
              history: await client.history(agentId, {
                throughCursor: cursor,
                limit: 200,
                toolOutput: true,
                signal: pending.signal,
              }),
            }
          : {
              approvals: await client.state(agentId, {
                throughCursor: through,
                afterApprovalCursor: cursor,
                signal: pending.signal,
              }),
            };
      await authorize?.(pending.signal);
      pending.signal.throwIfAborted();
      setView({ basis, mode, ...result });
    } catch (error) {
      if (!pending.signal.aborted)
        setView({
          basis,
          mode,
          error: error instanceof Error ? error.message : 'Activity could not be loaded.',
        });
    }
  };
  if (client === undefined) return null;
  const history = current?.history;
  const approvals = current?.approvals;
  const displayed =
    history !== undefined
      ? conversationFromWindow(history)
      : approvals !== undefined
        ? conversationFromCurrentState(approvals)
        : undefined;
  return (
    <>
      <div className="agent-surface__history-notice">
        {(conversation.historyStart ?? 0) > 0 || conversation.displayLimited ? (
          <>
            <span>Showing recent activity. Earlier recorded activity is available separately.</span>{' '}
            <Button
              size="sm"
              disabled={disabled}
              onClick={() => void read('history', conversation.historyPreviousCursor)}
            >
              Earlier activity
            </Button>
          </>
        ) : null}
        {conversation.approvalsTruncated ? (
          <Button size="sm" disabled={disabled} onClick={() => void read('approvals')}>
            Browse pending approvals
          </Button>
        ) : null}
      </div>
      <Modal
        open={current !== undefined}
        onClose={close}
        title={current?.mode === 'approvals' ? 'Pending approvals' : 'Earlier activity'}
        description={
          current?.mode === 'history'
            ? 'Each page shows a portion of recorded activity. The live conversation continues in the background.'
            : 'Approvals are checked against current Agent state. Refresh if the Agent changes.'
        }
        footer={
          <>
            {history?.previous_cursor ? (
              <Button
                disabled={current?.loading}
                onClick={() => void read('history', history.previous_cursor!)}
              >
                Earlier
              </Button>
            ) : null}
            {approvals?.next_approval_cursor ? (
              <Button
                disabled={current?.loading}
                onClick={() =>
                  void read('approvals', approvals.next_approval_cursor!, approvals.through_cursor)
                }
              >
                Next approvals
              </Button>
            ) : null}
            <Button
              disabled={current?.loading}
              onClick={() => void read(current?.mode ?? 'history')}
            >
              {current?.mode === 'approvals' ? 'Refresh approvals' : 'Latest recorded activity'}
            </Button>
            <Button onClick={close}>Return to conversation</Button>
          </>
        }
      >
        {current?.loading ? <p role="status">Loading activity…</p> : null}
        {current?.error ? <p role="alert">{current.error}</p> : null}
        {displayed === undefined ? null : (
          <MessageStream
            conversation={{ ...displayed, activeRunId: undefined }}
            emptyMessage="No activity in this page."
            arrivalsEnabled={false}
            approvalReadOnly={history !== undefined || approvalReadOnly}
            approvalBusy={approvalBusy}
            onApproval={(id, decision) => {
              close();
              onApproval(id, decision);
            }}
          />
        )}
      </Modal>
    </>
  );
}
