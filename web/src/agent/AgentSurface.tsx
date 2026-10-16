import { Presence } from '../ui/Presence';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { useConnectionGate } from '../connections/useConnectionGate';
import { subscriptionService } from '../connections/client';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import type { AgentController, AgentResourceState } from './controller';
import { Composer } from './Composer';
import type { AgentDraft } from './draft';
import { MessageStream } from './MessageStream';
import { ActiveThinking } from './ActiveThinking';
import './Agent.css';
import { AgentHeader } from './AgentHeader';
import { ProvisioningProgress } from '../sandbox/ProvisioningProgress';
import { agentProgress } from '../sandbox/progress';
import type { SteeringMode } from './SteeringMode';
import { BrandLoading } from '../landing/LandingCharacter';
import { AgentSandboxStatus } from './AgentSandboxStatus';
import { AgentSourceLabel, useAgentSource } from './source';
import { useAgentProvenance, useOwnerCreationLabel } from './provenance';
import { QueuedMessages } from './QueuedMessages';
import type { UploadAttachment } from './attachments';
import { AgentAccessRequests } from './AccessRequestDialog';
import type { CmaWebClient } from '../api/client';
import { AgentLifecycleFollowup } from '../create/AgentLifecycleFollowup';
import { useAgentConnectionReadiness } from './connectionReadiness';
import { useOptimisticMessages } from './optimisticMessages';
import { queuePresentation } from './queuePresentation';
import type { AgentViewPosition } from './viewPosition';
import type { MessageUser } from './MessageIdentity';
import { agentDisplayTitle } from './presentation';

interface AgentSurfaceProps {
  readonly lifecycleClient?: CmaWebClient;
  readonly accessUncertain?: boolean;
  /** Current personal access was checked for this exact Agent before mounting. */
  readonly accessAuthorized?: boolean;
  readonly draft?: AgentDraft;
  readonly viewerId?: string;
  readonly user?: MessageUser;
  readonly agentId: string;
  readonly controller: AgentController;
  readonly viewPosition?: AgentViewPosition;
  readonly detailsOpen: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly onNavigate: (route: AppRoute) => void;
}

function SandboxLink({
  id,
  name,
  onNavigate,
}: {
  readonly id: string;
  readonly name: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const route = { kind: 'sandbox', sandboxId: id } as const;
  return (
    <a
      href={routePath(route)}
      onClick={(event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
          return;
        event.preventDefault();
        onNavigate(route);
      }}
    >
      {name}
    </a>
  );
}

export function AgentSurface({
  lifecycleClient,
  draft,
  agentId,
  controller,
  viewPosition,
  detailsOpen,
  onDetailsOpenChange,
  onNavigate,
  viewerId,
  user,
  accessUncertain = false,
  accessAuthorized = false,
}: AgentSurfaceProps) {
  const [steeringMode, setSteeringMode] = useState<SteeringMode>('queue');
  const headingRef = useRef<HTMLHeadingElement>(null);
  const state: AgentResourceState =
    controller.state.status === 'ready' && controller.state.detail.id !== agentId
      ? { status: 'loading' }
      : controller.state;
  const documentTitle =
    state.status === 'ready' ? `${agentDisplayTitle(state.detail)} · CMA` : undefined;
  const source = useAgentSource(agentId, state.status === 'ready' && !accessUncertain);
  const provenance = useAgentProvenance(agentId, state.status === 'ready' && !accessUncertain);
  const sandboxOwner = controller.progressObservation?.sandbox.owner;
  const originLabel = useOwnerCreationLabel(provenance, sandboxOwner, state.status === 'ready');
  const readOnly =
    accessUncertain ||
    (viewerId !== undefined && sandboxOwner !== undefined && viewerId !== sandboxOwner);

  const connection = useAgentConnectionReadiness(
    agentId,
    viewerId,
    lifecycleClient,
    !readOnly && (state.status === 'ready' || (accessAuthorized && state.status !== 'error')),
  );
  const verified = connection.verified;
  const connectionError = connection.error;
  const connections = useConnectionGate(!readOnly, connection.scope);
  const optimistic = useOptimisticMessages(
    connection.scope,
    !readOnly && state.status === 'ready',
    state.status === 'ready' ? state.conversation : undefined,
    state.status === 'ready' && state.v2?.owner === (viewerId ?? verified?.owner)
      ? state.v2
      : undefined,
  );
  const discardPrepared = connection.discardPrepared;
  useLayoutEffect(() => {
    discardPrepared();
  }, [detailsOpen, discardPrepared]);
  const connectionConfiguration =
    state.status === 'ready'
      ? JSON.stringify([state.detail.sandbox.id, state.detail.sandbox.profile, state.detail.reach])
      : undefined;
  const observedConfiguration = useRef<{ scope: object; value: string } | undefined>(undefined);
  useLayoutEffect(() => {
    if (connectionConfiguration === undefined) return;
    if (
      observedConfiguration.current?.scope === connection.scope &&
      observedConfiguration.current.value !== connectionConfiguration
    )
      discardPrepared();
    observedConfiguration.current = { scope: connection.scope, value: connectionConfiguration };
  }, [connectionConfiguration, connection.scope, discardPrepared]);

  useEffect(() => {
    if (state.status !== 'ready') headingRef.current?.focus();
  }, [agentId, state.status]);

  useEffect(() => {
    if (documentTitle === undefined) return;
    document.title = documentTitle;
    return () => {
      document.title = 'CMA';
    };
  }, [documentTitle]);

  if (state.status === 'idle' || state.status === 'loading') {
    return (
      <section className="agent-surface agent-surface--loading" aria-busy="true">
        <header className="agent-header">
          <p className="agent-header__lineage">CMA</p>
          <h1 ref={headingRef} tabIndex={-1}>
            Agent
          </h1>
        </header>
        <div className="agent-loading">
          <BrandLoading label="Opening your agent" />
        </div>
      </section>
    );
  }

  if (state.status === 'error') {
    return (
      <section className="agent-surface agent-surface--error" role="alert">
        <div>
          <p className="eyebrow">Agent</p>
          <h1 ref={headingRef} tabIndex={-1}>
            Agent unavailable
          </h1>
          <p>{state.message}</p>
          <Button variant="outline" onClick={controller.reload}>
            Try again
          </Button>
        </div>
      </section>
    );
  }

  const { detail, conversation } = state;
  const messageQueue = queuePresentation(
    conversation,
    optimistic.messages,
    controller.queuedInputs,
  );
  const status = detail.status;
  const title = agentDisplayTitle(detail);
  const pausedNotice =
    conversation.livePausedNotice === undefined
      ? detail.pausedNotice
      : (conversation.livePausedNotice ?? undefined);
  const running = conversation.activeRunId !== undefined;
  const starting = controller.pending === 'send' && !running;
  const historyIncomplete = state.historyLoading === true || state.historyUnavailable === true;
  const approvalBusy =
    controller.pending === 'approval' ||
    historyIncomplete ||
    !controller.inputAvailability.allowed ||
    controller.controlsUnavailable === true;
  const issue = state.historyLoading
    ? 'Loading conversation activity.'
    : state.historyUnavailable
      ? 'Conversation activity is unavailable. Reconnecting.'
      : (connectionError ?? connections.error ?? controller.actionError);
  const preparation = agentProgress({ ...detail, status }, state.sandboxEvents ?? []);
  const uploadAttachment: UploadAttachment | undefined =
    readOnly || verified === undefined || lifecycleClient?.uploadAgentAttachment === undefined
      ? undefined
      : async (file, options) => {
          const uploaded = await lifecycleClient.uploadAgentAttachment!(agentId, file, options);
          if (uploaded.agentId !== agentId || !uploaded.path.startsWith('/home/attachments/'))
            throw new Error('The upload could not be verified for this Agent. Retry the upload.');
          return uploaded;
        };

  return (
    <section className="agent-surface">
      {connections.dialog}
      <AgentAccessRequests
        key={`access:${agentId}`}
        agentId={agentId}
        owner={viewerId}
        granted={detail.reach}
        sandboxStatus={detail.sandbox.status}
        enabled={!readOnly && viewerId !== undefined}
      />
      <AgentHeader
        title={title}
        status={status}
        headingRef={headingRef}
        detailsOpen={detailsOpen}
        detailsUnavailable={accessUncertain}
        onDetailsOpenChange={(open) => {
          if (!accessUncertain) onDetailsOpenChange(open);
        }}
        lineage={
          <>
            <SandboxLink
              id={detail.sandbox.id}
              name={detail.sandbox.name}
              onNavigate={onNavigate}
            />
            <span aria-hidden="true">/</span>
            <span>{detail.runtimeMetadata?.workingDirectory || detail.workspace.path}</span>
            <span aria-hidden="true">/</span>
            <span>Agent</span>
          </>
        }
      >
        <AgentSourceLabel source={source} provenance={provenance} originLabel={originLabel} />
        {!readOnly &&
        verified?.services.some((service) => service.service === subscriptionService) ? (
          <Button
            onClick={() =>
              connections.run(
                { type: 'agent', id: agentId },
                () => {
                  controller.clearActionError();
                  controller.reload();
                },
                true,
              )
            }
          >
            Re-login with Codex subscription
          </Button>
        ) : null}
      </AgentHeader>

      <Presence>
        {issue === undefined ? null : (
          <div className="agent-action-error" role={historyIncomplete ? 'status' : 'alert'}>
            <span>{issue}</span>
            {historyIncomplete ? null : (
              <Button
                size="sm"
                icon={<IconCloseOutline16 />}
                aria-label="Dismiss error"
                onClick={() => {
                  connection.clearError();
                  connections.clearError();
                  controller.clearActionError();
                }}
              />
            )}
            {connectionError && !readOnly ? (
              <Button size="sm" onClick={connection.retry}>
                Retry connections
              </Button>
            ) : null}
          </div>
        )}
      </Presence>

      {controller.pendingMessage === undefined && !controller.pendingControl ? null : (
        <div className="agent-action-error" role="status">
          <span>
            {controller.pendingMessage !== undefined
              ? 'Your message is awaiting confirmation. Recover it before sending another message.'
              : 'Your response action is awaiting confirmation.'}
          </span>
          <Button
            size="sm"
            disabled={
              readOnly ||
              controller.pending !== undefined ||
              historyIncomplete ||
              (status === 'starting' && !controller.inputAvailability.allowed)
            }
            onClick={() => void controller.recoverPending?.()}
          >
            Recover pending request
          </Button>
        </div>
      )}

      {controller.controlNotice === undefined ? null : (
        <div className="agent-action-error" role="status">
          {controller.controlNotice}
        </div>
      )}

      {!readOnly && lifecycleClient !== undefined && viewerId !== undefined ? (
        <AgentLifecycleFollowup client={lifecycleClient} viewerId={viewerId} agentId={agentId} />
      ) : null}

      <div className="agent-surface__conversation">
        {status === 'starting' ||
        detail.failure !== undefined ||
        preparation.some((step) => step.status !== 'Done') ? (
          <ProvisioningProgress
            title="Agent preparation"
            steps={preparation}
            failure={detail.failure}
          />
        ) : null}
        <MessageStream
          user={user}
          viewPosition={viewPosition}
          conversation={conversation}
          optimisticMessages={messageQueue.messages}
          arrivalsEnabled={!historyIncomplete && !accessUncertain}
          approvalBusy={approvalBusy}
          approvalReadOnly={readOnly}
          onApproval={(id, decision) => void controller.approve(id, decision)}
        >
          {controller.queuedInputs === undefined ? null : (
            <QueuedMessages
              items={messageQueue.items}
              disabled={readOnly || historyIncomplete || !controller.inputAvailability.allowed}
              onEdit={controller.queuedInputs.edit}
              onRemove={controller.queuedInputs.remove}
              onRecover={controller.queuedInputs.recover}
              uploadAttachment={uploadAttachment}
              user={user}
            />
          )}
          {controller.queuedInputs?.error ? (
            <p className="queued-messages__error" role="status">
              {controller.queuedInputs.error}{' '}
              <Button size="sm" onClick={controller.queuedInputs.refresh}>
                Refresh queue
              </Button>
            </p>
          ) : null}
          <AgentSandboxStatus
            observation={controller.progressObservation}
            onNavigate={onNavigate}
          />
        </MessageStream>
      </div>
      <div className="agent-surface__input">
        <ActiveThinking key={agentId} conversation={conversation} />
        <Composer
          showUsage
          usage={conversation.usage}
          steeringMode={steeringMode}
          onSteeringModeChange={state.v2 === undefined ? undefined : setSteeringMode}
          draft={draft}
          availableCommands={readOnly ? undefined : conversation.availableCommands}
          key={`composer:${agentId}`}
          quickMessages={
            source.status === 'ready' && source.value.access !== 'shared'
              ? source.value.trigger?.quick_messages
              : undefined
          }
          status={status}
          pausedNotice={pausedNotice}
          running={running}
          queueWhileRunning={state.v2 !== undefined}
          queueWhileStarting={state.v2 !== undefined}
          starting={starting}
          controlsUnavailable={controller.controlsUnavailable}
          requestPending={
            controller.pendingMessage !== undefined || controller.pendingControl === true
          }
          acceptedMessage={controller.acceptedMessage}
          acceptedSteeringMessage={controller.acceptedSteeringMessage}
          acceptedInterruptionMessage={controller.acceptedInterruptionMessage}
          uploadAttachment={uploadAttachment}
          inputAvailability={
            readOnly
              ? {
                  ...controller.inputAvailability,
                  allowed: false,
                  notice: accessUncertain
                    ? controller.inputAvailability.notice
                    : 'Only the owner can send messages to this agent.',
                }
              : verified === undefined
                ? { ...controller.inputAvailability, allowed: false }
                : controller.inputAvailability
          }
          busy={
            connections.pending ||
            historyIncomplete ||
            (controller.pending !== undefined && controller.pending !== 'send')
          }
          onSend={(content, onAccepted) => {
            if (readOnly || verified === undefined) return;
            const message = optimistic.begin(
              content,
              running ? (steeringMode === 'queue' ? 'queue' : 'interrupt') : undefined,
              controller.queuedInputs?.items.map((item) => item.commandId),
            );
            return new Promise<void>((resolve) => {
              const settled = () => {
                message.settled();
                resolve();
              };
              const cancelled = () => {
                message.cancelled();
                resolve();
              };
              const send = () => {
                const submit =
                  running && steeringMode === 'interrupt'
                    ? controller.interruptWithMessage
                    : controller.send;
                if (submit === undefined) {
                  cancelled();
                  return;
                }
                const args: Parameters<AgentController['send']> = [
                  content,
                  (receipt) => {
                    if (message.accepted(receipt)) onAccepted();
                    resolve();
                  },
                  message.observed,
                  cancelled,
                ];
                const request =
                  running && steeringMode === 'interrupt'
                    ? submit(...args, message.ordinaryInput)
                    : submit(...args);
                void request.then(settled, settled);
              };
              if (
                state.v2?.owner === (viewerId ?? verified.owner) &&
                controller.resourceAvailability.state === 'live'
              ) {
                // Transport still rechecks identity before admission and acceptance.
                send();
              } else {
                void connections.checkAndRun(
                  { type: 'agent', id: agentId },
                  viewerId ?? verified.owner,
                  send,
                  state.v2 !== undefined && !detailsOpen ? connection.takePrepared : undefined,
                  cancelled,
                );
              }
            });
          }}
          onStop={controller.interrupt}
        />
      </div>
      {verified === undefined && connection.needed && !readOnly ? (
        <ConnectionWizard
          target={{ type: 'agent', id: agentId }}
          presentation="dialog"
          expectedOwner={viewerId}
          onReady={connection.accept}
          onCancel={() => onNavigate({ kind: 'home' })}
        />
      ) : null}
    </section>
  );
}
