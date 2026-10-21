import { useEffect, useMemo, useRef, useState } from 'react';
import type { SharedAgent as SharedAgentDetail } from '@chronoai/cma-sdk';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import { CommandKeys } from '../api/client';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import {
  hydrateSharedAgent,
  loadSharedAgent,
  SharedAgentAccessChangedError,
} from './loadSharedAgent';
import { MessageStream } from '../agent/MessageStream';
import { Composer } from '../agent/Composer';
import { foldLiveEvent, type ConversationState } from '../agent/conversation';
import { useAgentProvenance, creationLabel } from '../agent/provenance';
import { Button } from '../ui/Button';
import { AgentHeader } from '../agent/AgentHeader';
import { routePath, type AppRoute } from '../routing/routes';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { Alert } from '../ui/Alert';
import { SessionSettings } from '../agent/SessionSettings';
import type { UploadAttachment } from '../agent/attachments';
import type { ApprovalDecision } from '../agent/controller';
import { agentDisplayTitle } from '../agent/presentation';
import '../agent/Agent.css';
import './AgentShareLinks.css';

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
          (error instanceof SharedAgentAccessChangedError ||
            (error instanceof ApiError && [401, 403, 404, 409].includes(error.status)))
        )
          onAccessChanged();
        throw error;
      }
    },
    [web, client, agentId, onAccessChanged],
  );
  const history = useAuthorizedRead(read);
  const ready = history.state.status === 'ready' ? history.state.value : undefined;
  const readyRef = useRef(ready);
  useEffect(() => {
    readyRef.current = ready;
  }, [ready]);
  // Renewing unchanged access must not restart a healthy stream or replace its
  // rich state with the transcript-only snapshot. Losing access changes the
  // basis immediately, even while its next authorization read is pending.
  const authority =
    ready === undefined
      ? undefined
      : JSON.stringify([
          ready.access.policy_revision,
          [...ready.access.capabilities].sort(),
          ready.access.shared_by?.subject,
        ]);
  const basis = useMemo(
    () => (authority === undefined ? undefined : { web, client, agentId, authority }),
    [web, client, agentId, authority],
  );
  const [live, setLive] = useState<{
    readonly basis: typeof basis;
    readonly conversation: ConversationState;
  }>();
  const [hydrationRevision, setHydrationRevision] = useState(0);
  const [historyError, setHistoryError] = useState<{
    readonly basis: typeof basis;
    readonly revision: number;
    readonly message: string;
  }>();
  const historyComplete = basis !== undefined && live?.basis === basis;
  const [command, setCommand] = useState<AbortController>();
  const busy = command !== undefined && !command.signal.aborted;
  const [error, setError] = useState<string>();
  const [keys] = useState(() => new CommandKeys());
  const mutation = useRef<AbortController | undefined>(undefined);
  const refreshHistory = history.reload;
  const provenance = useAgentProvenance(agentId, ready !== undefined);
  useEffect(() => {
    const snapshot = readyRef.current;
    if (basis === undefined || snapshot === undefined) {
      mutation.current?.abort();
      mutation.current = undefined;
      return;
    }
    const stream = new AbortController();
    let retry: ReturnType<typeof setTimeout> | undefined;
    let hydrated = false;
    void hydrateSharedAgent(web, client, snapshot, stream.signal)
      .then(async (conversation) => {
        if (stream.signal.aborted) return;
        hydrated = true;
        setLive({ basis, conversation });
        await web.replayAgent(agentId, {
          after: conversation.cursor,
          signal: stream.signal,
          onEvent: (event) => {
            if (stream.signal.aborted) return;
            setLive((current) => ({
              basis,
              conversation: foldLiveEvent(
                current?.basis === basis ? current.conversation : conversation,
                event,
                agentId,
              ),
            }));
          },
        });
      })
      .then(() => {
        if (!stream.signal.aborted) retry = setTimeout(refreshHistory, 5_000);
      })
      .catch((cause: unknown) => {
        if (stream.signal.aborted) return;
        if (
          hydrated ||
          cause instanceof SharedAgentAccessChangedError ||
          (cause instanceof ApiError && [401, 403, 404, 409].includes(cause.status))
        ) {
          onAccessChanged();
          return;
        }
        setHistoryError({
          basis,
          revision: hydrationRevision,
          message: 'Conversation activity could not be loaded. Your recorded messages are shown.',
        });
      });
    return () => {
      stream.abort();
      clearTimeout(retry);
    };
  }, [web, client, agentId, basis, refreshHistory, onAccessChanged, hydrationRevision]);
  useEffect(() => () => mutation.current?.abort(), []);

  const can = (capability: string) => ready?.access.capabilities.includes(capability) === true;
  const authorizeSettings = async (signal: AbortSignal) => {
    if (!historyComplete) throw new Error('Agent activity is still loading. Try again shortly.');
    try {
      const current = await client.resourceAccess.effective('agent', agentId, { signal });
      signal.throwIfAborted();
      if (!current.capabilities.includes('operate_agent'))
        throw new Error('Your access no longer permits changing Agent settings.');
    } catch (cause) {
      if (!signal.aborted) onAccessChanged();
      throw cause;
    }
  };
  const uploadAttachment: UploadAttachment | undefined =
    !historyComplete || !can('operate_agent') || web.uploadAgentAttachment === undefined
      ? undefined
      : async (file, options) => {
          const access = await client.resourceAccess.effective('agent', agentId, options);
          if (!access.capabilities.includes('operate_agent')) {
            onAccessChanged();
            throw new Error('Your access no longer permits uploading files.');
          }
          return web.uploadAgentAttachment!(agentId, file, options);
        };
  const run = async (
    capability: string,
    logicalKey: string,
    action: (signal: AbortSignal, idempotencyKey: string) => Promise<unknown>,
  ) => {
    if (mutation.current !== undefined || !historyComplete || !can(capability)) return;
    const request = new AbortController();
    mutation.current = request;
    setCommand(request);
    setError(undefined);
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
    live !== undefined && live.basis === basis ? live.conversation : ready?.conversation;
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
        menu={[{ type: 'label', id: 'creation-origin', text: creationLabel(provenance) }]}
      />
      <div className="agent-surface__alerts">
        {ready?.access.shared_by ? (
          <div className="shared-agent__attribution" role="note">
            Shared by {ready.access.shared_by.display_name?.trim() || 'the Agent owner'}
            <span>· {can('operate_agent') ? 'View and write access' : 'View-only access'}</span>
          </div>
        ) : null}
        {history.state.status === 'loading' ? (
          <Alert tone="info">Loading authorized Agent history</Alert>
        ) : null}
        {history.state.status === 'error' ? (
          <Alert
            tone="error"
            actions={<Button onClick={onAccessChanged}>Check access again</Button>}
          >
            {history.state.message}
          </Alert>
        ) : null}
        {historyError?.basis === basis && historyError?.revision === hydrationRevision ? (
          <Alert
            tone="error"
            actions={
              <Button onClick={() => setHydrationRevision((revision) => revision + 1)}>
                Retry conversation activity
              </Button>
            }
          >
            {historyError.message}
          </Alert>
        ) : null}
        {error === undefined ? null : <Alert tone="error">{error}</Alert>}
      </div>
      <div className="agent-surface__conversation">
        {conversation === undefined ? null : (
          <MessageStream
            unknownUserName="Participant"
            emptyMessage="No messages yet."
            conversation={conversation}
            approvalBusy={busy}
            approvalReadOnly={!historyComplete || !can('resolve_approval')}
            arrivalsEnabled={historyComplete}
            onApproval={approve}
          />
        )}
      </div>
      {ready === undefined ? null : can('interact') ? (
        <div className="agent-surface__input">
          <Composer
            settings={
              can('operate_agent') ? (
                <SessionSettings
                  agentId={agentId}
                  active
                  busy={busy || !historyComplete || status !== 'ready'}
                  authorizeUpdate={authorizeSettings}
                />
              ) : undefined
            }
            uploadAttachment={uploadAttachment}
            status={status}
            running={conversation?.activeRunId !== undefined}
            busy={busy || !historyComplete}
            inputAvailability={{ allowed: historyComplete }}
            onSend={async (text, accepted) => {
              const result = await run('interact', `input:${text}`, (signal, idempotencyKey) =>
                client.agents.input(agentId, { text }, { signal, idempotencyKey }),
              );
              if (result !== undefined) {
                accepted();
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
