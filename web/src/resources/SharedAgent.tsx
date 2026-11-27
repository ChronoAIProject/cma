import { useEffect, useMemo, useRef, useState } from 'react';
import type { SharedAgent as SharedAgentDetail } from '@cma/client';
import { ApiError } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { CommandKeys } from '../api/client';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import {
  hydrateSharedAgent,
  loadSharedAgentAccess,
  loadSharedAgentHead,
  loadSharedAgentTranscript,
  sharedAgentAuthority,
  SharedAgentAccessChangedError,
} from './loadSharedAgent';
import { MessageStream } from '../agent/MessageStream';
import { Composer } from '../agent/Composer';
import { conversationFromTranscript, type ConversationState } from '../agent/conversation';
import { HydrationEvents } from '../agent/mergeHydration';
import { useAgentProvenance, creationLabel } from '../agent/provenance';
import { Button } from '../ui/Button';
import { AgentHeader } from '../agent/AgentHeader';
import { routePath, type AppRoute } from '../routing/routes';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { Alert } from '../ui/Alert';
import { SessionSettings } from '../agent/SessionSettings';
import { agentInitialized } from '../agent/modelReadiness';
import type { UploadAttachment } from '../agent/attachments';
import type { ApprovalDecision } from '../agent/controller';
import { agentDisplayTitle } from '../agent/presentation';
import '../agent/Agent.css';
import './AgentShareLinks.css';

interface Props {
  readonly authEpoch?: number;
  readonly viewerId?: string;
  readonly computerObservation?: AgentComputerObservation;
  readonly client: ResourceViewsClient;
  readonly web: CmaWebClient;
  readonly detail: SharedAgentDetail;
  readonly onAccessChanged: () => void;
  readonly filesOpen?: boolean;
  readonly onFilesOpenChange?: (open: boolean) => void;
  readonly detailsOpen: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly onNavigate?: (route: AppRoute) => void;
}

export function SharedAgent({
  authEpoch,
  viewerId,
  client,
  web,
  detail,
  onAccessChanged,
  filesOpen,
  onFilesOpenChange,
  detailsOpen,
  onDetailsOpenChange,
  onNavigate,
  computerObservation,
}: Props) {
  const agentId = detail.id;
  const title = agentDisplayTitle(detail);
  useDocumentTitle(title, 'task');
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        return await loadSharedAgentAccess(client, agentId, signal);
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
    [client, agentId, onAccessChanged],
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
  const authority = ready === undefined ? undefined : sharedAgentAuthority(ready.access);
  const basis = useMemo(
    () => (authority === undefined ? undefined : { web, client, agentId, authority }),
    [web, client, agentId, authority],
  );
  const [live, setLive] = useState<{
    readonly basis: typeof basis;
    readonly conversation: ConversationState;
    readonly historyLoaded: boolean;
  }>();
  const [hydrationRevision, setHydrationRevision] = useState(0);
  const observed = basis !== undefined && live?.basis === basis ? live.conversation : undefined;
  const observedTools = observed?.toolCalls;
  const observedRun = observed?.activeRunId;
  const observedLoaded = live?.basis === basis && live?.historyLoaded === true;
  useLayoutEffect(() => {
    computerObservation?.publish(observedTools, observedRun, observedLoaded);
    return () => computerObservation?.publish(undefined);
  }, [computerObservation, observedTools, observedRun, observedLoaded]);
  const [historyError, setHistoryError] = useState<{
    readonly basis: typeof basis;
    readonly revision: number;
    readonly message: string;
  }>();
  const liveRef = useRef(live);
  useEffect(() => {
    liveRef.current = live;
  }, [live]);
  const [command, setCommand] = useState<AbortController>();
  const busy = command !== undefined && !command.signal.aborted;
  const [error, setError] = useState<string>();
  const [keys] = useState(() => new CommandKeys());
  const mutation = useRef<AbortController | undefined>(undefined);
  const provenance = useAgentProvenance(agentId, ready !== undefined);
  useEffect(() => {
    const authorization = readyRef.current;
    if (basis === undefined || authorization === undefined) {
      mutation.current?.abort();
      mutation.current = undefined;
      return;
    }
    const stream = new AbortController();
    let retry: ReturnType<typeof setTimeout> | undefined;
    let buffering = true;
    let historyLoaded = false;
    let conversation =
      liveRef.current?.basis === basis
        ? liveRef.current.conversation
        : conversationFromTranscript({ agentId, turns: [], throughSeq: 0 });
    const buffered = new HydrationEvents();
    let replayStarted = false;
    let liveBaseline = 0;
    const publish = () => setLive({ basis, conversation, historyLoaded });
    const accessFailure = (cause: unknown) =>
      cause instanceof SharedAgentAccessChangedError ||
      (cause instanceof ApiError && [401, 403, 404, 409].includes(cause.status));
    const replay = async () => {
      if (stream.signal.aborted || replayStarted) return;
      replayStarted = true;
      buffered.replayFrom(conversation.cursor);
      try {
        stream.signal.throwIfAborted();
        await web.replayAgent(agentId, {
          after: conversation.cursor,
          signal: stream.signal,
          onEvent: (event) => {
            if (stream.signal.aborted) return;
            conversation = buffered.apply(conversation, event, agentId, buffering);
            publish();
          },
        });
      } catch (cause) {
        if (stream.signal.aborted) return;
        if (accessFailure(cause)) {
          onAccessChanged();
          return;
        }
      }
      // A durable message can span several replay frames with the same cursor.
      // Refresh its snapshot after disconnect instead of skipping a partially
      // delivered message by reconnecting after the last frame's cursor.
      if (!stream.signal.aborted)
        retry = setTimeout(() => setHydrationRevision((revision) => revision + 1), 5_000);
    };
    void loadSharedAgentHead(client, authorization, stream.signal)
      .then((head) => {
        if (stream.signal.aborted || replayStarted || head === undefined) return;
        liveBaseline = Number(head.through_cursor.split('.').at(-1));
        if (head.active_turn !== null) {
          conversation = buffered.apply(
            conversation,
            {
              type: 'RUN_STARTED',
              threadId: agentId,
              runId: String(head.active_turn.sequence),
              cursor: head.active_turn.sequence,
              timestamp: head.active_turn.appended_at,
            },
            agentId,
            true,
          );
        } else {
          conversation = buffered.apply(
            conversation,
            { type: 'RUN_FINISHED', threadId: agentId, runId: '', cursor: liveBaseline },
            agentId,
            true,
          );
        }
        // The SDK validates this public cursor; it starts only the live tail and
        // never acknowledges or skips the independent historical page reads.
        conversation = { ...conversation, cursor: liveBaseline };
        publish();
        void replay();
      })
      .catch((cause: unknown) => {
        if (!stream.signal.aborted && accessFailure(cause)) onAccessChanged();
        // Older clients or a temporary head outage retain transcript-first replay.
      });
    void loadSharedAgentTranscript(web, client, authorization, stream.signal)
      .then(({ value: snapshot }) => {
        if (stream.signal.aborted) return;
        conversation = buffered.merge(snapshot.conversation, agentId, false);
        publish();
        // Either the bounded head or transcript starts replay. Rich history is bounded
        // to that exact snapshot; replay its buffered new frames once onto the
        // rebuilt state so late history cannot replace newer messages or controls.
        void replay();
        void hydrateSharedAgent(
          web,
          client,
          snapshot,
          stream.signal,
          Math.max(snapshot.conversation.cursor, liveBaseline),
        )
          .then((rebuilt) => {
            if (stream.signal.aborted) return;
            conversation = buffered.merge(rebuilt, agentId, true);
            buffering = false;
            historyLoaded = true;
            publish();
          })
          .catch((cause: unknown) => {
            if (stream.signal.aborted) return;
            buffering = false;
            buffered.discardBuffered();
            if (accessFailure(cause)) {
              onAccessChanged();
              return;
            }
            setHistoryError({
              basis,
              revision: hydrationRevision,
              message:
                'Conversation activity could not be loaded. Your recorded messages are shown.',
            });
          });
      })
      .catch((cause: unknown) => {
        if (stream.signal.aborted) return;
        buffering = false;
        buffered.discardBuffered();
        if (accessFailure(cause)) {
          onAccessChanged();
          return;
        }
        setHistoryError({
          basis,
          revision: hydrationRevision,
          message: 'Conversation history could not be loaded. Retry to show recorded messages.',
        });
      });
    return () => {
      stream.abort();
      clearTimeout(retry);
      mutation.current?.abort();
      mutation.current = undefined;
    };
  }, [web, client, agentId, basis, onAccessChanged, hydrationRevision]);
  useEffect(() => () => mutation.current?.abort(), []);

  const can = (capability: string) => ready?.access.capabilities.includes(capability) === true;
  const authorizeSettings = async (signal: AbortSignal) => {
    if (ready === undefined) throw new Error('Current Agent access is unavailable.');
    try {
      const current = await client.resourceAccess.effective('agent', agentId, { signal });
      signal.throwIfAborted();
      if (
        sharedAgentAuthority(current) !== authority ||
        !current.capabilities.includes('operate_agent')
      )
        throw new Error('Your access no longer permits changing Agent settings.');
    } catch (cause) {
      if (!signal.aborted) onAccessChanged();
      throw cause;
    }
  };
  const uploadAttachment: UploadAttachment | undefined =
    !can('operate_agent') || web.uploadAgentAttachment === undefined
      ? undefined
      : async (file, options) => {
          const access = await client.resourceAccess.effective('agent', agentId, options);
          options?.signal?.throwIfAborted();
          if (
            sharedAgentAuthority(access) !== authority ||
            !access.capabilities.includes('operate_agent')
          ) {
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
    if (mutation.current !== undefined || !can(capability)) return;
    const request = new AbortController();
    mutation.current = request;
    setCommand(request);
    setError(undefined);
    try {
      const current = await client.resourceAccess.effective('agent', agentId, {
        signal: request.signal,
      });
      request.signal.throwIfAborted();
      if (
        sharedAgentAuthority(current) !== authority ||
        !current.capabilities.includes(capability)
      ) {
        onAccessChanged();
        throw new Error('Current access does not permit this action.');
      }
      const result = await action(request.signal, keys.for(logicalKey));
      if (request.signal.aborted) return;
      keys.settle(logicalKey);
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
  const conversation = live !== undefined && live.basis === basis ? live.conversation : undefined;
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
        readOnly={
          ready !== undefined &&
          !can('interact') &&
          !can('operate_agent') &&
          !can('resolve_approval')
        }
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
        filesOpen={filesOpen}
        onFilesOpenChange={onFilesOpenChange}
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
          <Alert tone="info">Checking Agent access</Alert>
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
            retainedOutput={
              viewerId && basis ? { client: web, agentId, viewerId, authority: basis } : undefined
            }
            deliverables={
              web.deliverables && viewerId && basis
                ? { agentId, viewerId, client: web.deliverables, authority: basis }
                : undefined
            }
            media={
              web.files && can('view_files')
                ? {
                    agentId,
                    files: web.files,
                    workingDirectory: conversation.runtimeMetadata?.workingDirectory,
                  }
                : undefined
            }
            unknownUserName="Participant"
            emptyMessage="No messages yet."
            conversation={conversation}
            turnPresentation={
              observedLoaded && authEpoch !== undefined ? { status, authEpoch } : undefined
            }
            approvalBusy={busy}
            approvalReadOnly={!can('resolve_approval')}
            arrivalsEnabled={ready !== undefined}
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
                  usage={conversation?.usage}
                  agentId={agentId}
                  viewerId={viewerId}
                  active
                  runtimeReady={agentInitialized(status, detail.ready_at)}
                  busy={busy || status !== 'ready'}
                  authorizeUpdate={authorizeSettings}
                />
              ) : undefined
            }
            uploadAttachment={uploadAttachment}
            status={status}
            running={conversation?.activeRunId !== undefined}
            queueWhileRunning
            busy={busy}
            inputAvailability={{ allowed: true }}
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
import { useLayoutEffect } from 'react';
import type { AgentComputerObservation } from '../agent/environment/observation';
