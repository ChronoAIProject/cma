import { useEffect, useMemo, useRef, useState } from 'react';
import type { SharedAgent as SharedAgentDetail } from '@chronoai/cma-sdk';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import { CommandKeys } from '../api/client';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { loadSharedAgent } from './loadSharedAgent';
import { MessageStream } from '../agent/MessageStream';
import { Composer } from '../agent/Composer';
import { foldLiveEvent, type ConversationState } from '../agent/conversation';
import { useAgentProvenance, creationLabel } from '../agent/provenance';
import { Button } from '../ui/Button';
import { AgentHeader } from '../agent/AgentHeader';
import { routePath, type AppRoute } from '../routing/routes';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { ActiveThinking } from '../agent/ActiveThinking';
import type { ApprovalDecision } from '../agent/controller';
import { agentDisplayTitle } from '../agent/presentation';
import '../agent/Agent.css';

interface Props {
  readonly client: ResourceViewsClient;
  readonly web: CmaWebClient;
  readonly detail: SharedAgentDetail;
  readonly onAccessChanged: () => void;
  readonly detailsOpen: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly onNavigate?: (route: AppRoute) => void;
}

export function SharedAgent({
  client,
  web,
  detail,
  onAccessChanged,
  detailsOpen,
  onDetailsOpenChange,
  onNavigate,
}: Props) {
  const agentId = detail.id;
  const title = agentDisplayTitle(detail);
  useDocumentTitle(title);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        return await loadSharedAgent(web, client, agentId, signal);
      } catch (error) {
        if (
          !signal.aborted &&
          error instanceof ApiError &&
          [401, 403, 404, 409].includes(error.status)
        )
          onAccessChanged();
        throw error;
      }
    },
    [web, client, agentId, onAccessChanged],
  );
  const history = useAuthorizedRead(read);
  const ready = history.state.status === 'ready' ? history.state.value : undefined;
  const [live, setLive] = useState<{
    readonly basis: typeof ready;
    readonly conversation: ConversationState;
  }>();
  const [command, setCommand] = useState<AbortController>();
  const busy = command !== undefined && !command.signal.aborted;
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [keys] = useState(() => new CommandKeys());
  const mutation = useRef<AbortController | undefined>(undefined);
  const refreshHistory = history.reload;
  const provenance = useAgentProvenance(agentId, ready !== undefined);
  useEffect(() => {
    if (ready === undefined) {
      mutation.current?.abort();
      mutation.current = undefined;
      return;
    }
    const stream = new AbortController();
    let retry: ReturnType<typeof setTimeout> | undefined;
    void web
      .replayAgent(agentId, {
        after: ready.conversation.cursor,
        signal: stream.signal,
        onEvent: (event) => {
          if (stream.signal.aborted) return;
          setLive((current) => ({
            basis: ready,
            conversation: foldLiveEvent(
              current?.basis === ready ? current.conversation : ready.conversation,
              event,
              new Date().toISOString(),
            ),
          }));
        },
      })
      .then(() => {
        if (!stream.signal.aborted) retry = setTimeout(refreshHistory, 5_000);
      })
      .catch(() => {
        if (!stream.signal.aborted) onAccessChanged();
      });
    return () => {
      stream.abort();
      clearTimeout(retry);
    };
  }, [web, agentId, ready, refreshHistory, onAccessChanged]);
  useEffect(() => () => mutation.current?.abort(), []);

  const can = (capability: string) => ready?.access.capabilities.includes(capability) === true;
  const run = async (
    capability: string,
    logicalKey: string,
    action: (signal: AbortSignal, idempotencyKey: string) => Promise<unknown>,
  ) => {
    if (mutation.current !== undefined || !can(capability)) return;
    const request = new AbortController();
    mutation.current = request;
    setCommand(request);
    setError(undefined);
    setNotice(undefined);
    try {
      const current = await client.resourceAccess.effective('agent', agentId, {
        signal: request.signal,
      });
      if (!current.capabilities.includes(capability)) {
        onAccessChanged();
        throw new Error('Current access does not permit this action.');
      }
      const result = await action(request.signal, keys.for(logicalKey));
      if (request.signal.aborted) return;
      keys.settle(logicalKey);
      refreshHistory();
      return result;
    } catch (caught) {
      if (!request.signal.aborted) {
        setError(caught instanceof Error ? caught.message : 'The Agent command did not complete.');
        if (caught instanceof ApiError && [401, 403, 404, 409, 503].includes(caught.status))
          onAccessChanged();
      }
      return undefined;
    } finally {
      if (mutation.current === request) mutation.current = undefined;
      if (!request.signal.aborted) setCommand(undefined);
    }
  };
  const approve = (id: string, decision: ApprovalDecision) => {
    void run('resolve_approval', `approval:${id}:${decision}`, (signal, idempotencyKey) =>
      client.agents.answerApproval(agentId, id, { decision }, { signal, idempotencyKey }),
    );
  };
  const conversation =
    live !== undefined && live.basis === ready ? live.conversation : ready?.conversation;
  const status =
    conversation?.agentStatus !== undefined &&
    conversation.agentStatusAt !== undefined &&
    Date.parse(conversation.agentStatusAt) > Date.parse(detail.updated_at)
      ? conversation.agentStatus
      : detail.status;
  return (
    <section className="agent-surface">
      <AgentHeader
        title={title}
        status={status}
        // A shared reader is authorized for this Agent, not for its parents,
        // so the breadcrumb names only the place it was opened from.
        breadcrumb={[
          {
            label: 'Agent Manager',
            href: routePath({ kind: 'agent_manager' }),
            onSelect:
              onNavigate === undefined ? undefined : () => onNavigate({ kind: 'agent_manager' }),
          },
          { label: title },
        ]}
        detailsOpen={detailsOpen}
        onDetailsOpenChange={onDetailsOpenChange}
      >
        <p className="agent-header__source">{creationLabel(provenance)}</p>
      </AgentHeader>
      <div className="agent-surface__conversation">
        {history.state.status === 'loading' ? (
          <p role="status">Loading authorized Agent history</p>
        ) : null}
        {history.state.status === 'error' ? (
          <div role="alert">
            <p>{history.state.message}</p>
            <Button onClick={onAccessChanged}>Check access again</Button>
          </div>
        ) : null}
        {conversation === undefined ? null : (
          <MessageStream
            unknownUserName="Participant"
            emptyMessage="No messages yet."
            conversation={conversation}
            approvalBusy={busy}
            approvalReadOnly={!can('resolve_approval')}
            onApproval={approve}
          />
        )}
      </div>
      {error === undefined ? null : (
        <p className="agent-action-error" role="alert">
          {error}
        </p>
      )}
      {notice === undefined ? null : (
        <p className="agent-action-error" role="status">
          {notice}
        </p>
      )}
      {ready === undefined ? null : can('interact') ? (
        <div className="agent-surface__input">
          {conversation === undefined ? null : <ActiveThinking conversation={conversation} />}
          <Composer
            status={status}
            running={conversation?.activeRunId !== undefined}
            busy={busy}
            inputAvailability={{ allowed: true }}
            onSend={async (text, accepted) => {
              const result = await run('interact', `input:${text}`, (signal, idempotencyKey) =>
                client.agents.input(agentId, { text }, { signal, idempotencyKey }),
              );
              if (result !== undefined) {
                accepted();
                setNotice('Message accepted.');
              }
            }}
            onStop={() =>
              run('interact', 'interrupt', (signal, idempotencyKey) =>
                client.agents.interrupt(agentId, { signal, idempotencyKey }),
              ).then(() => {})
            }
          />
        </div>
      ) : (
        <div className="agent-surface__input">
          <div className="agent-composer">
            <p className="agent-composer__notice">
              You can view this Agent. Your current access does not allow interaction.
            </p>
          </div>
        </div>
      )}
    </section>
  );
}
