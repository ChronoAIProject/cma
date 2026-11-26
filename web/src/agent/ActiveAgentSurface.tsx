import { ComputerContext } from './environment/context';
import { SuggestedReplies } from './SuggestedReplies';
import { useAgentSuggestions } from './useAgentSuggestions';
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useConnectionGate } from '../connections/useConnectionGate';
import { subscriptionService } from '../connections/client';
import type { AgentResourceState } from './controller';
import type { AgentSurfaceProps } from './AgentSurfaceProps';
import { Composer } from './Composer';
import { SessionSettings } from './SessionSettings';
import { modelCatalogReady } from './modelReadiness';
import { useMessageFeedback } from './useMessageFeedback';
import { OwnerMessageStream } from './OwnerMessageStream';
import './Agent.css';
import { OwnerAgentHeader } from './OwnerAgentHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { agentBreadcrumb, agentHeaderMenu, agentHeaderActionHandler } from './headerActions';
import { AgentStatusPanel } from './AgentStatusPanel';
import type { SteeringMode } from './SteeringMode';
import {
  AgentOpening,
  AgentUnavailable,
  AgentCommandNotices,
  AgentSurfaceIssue,
  AgentConnectionSetup,
} from './AgentSurfaceStates';
import { AgentSandboxStatus } from './AgentSandboxStatus';
import { setupEnvironmentRoute } from './setupEnvironment';
import { agentOriginEntries, useAgentSource } from './source';
import { useAgentProvenance, useOwnerCreationLabel } from './provenance';
import { AgentInputQueue } from './AgentInputQueue';
import type { AgentTransportJournal } from './transport/journal';
import { agentAttachmentUploader } from './attachmentUpload';
import { AgentAccessRequests } from './AccessRequestDialog';
import { useAgentConnectionReadiness } from './connectionReadiness';
import { useOptimisticMessages } from './optimisticMessages';
import { queuePresentation } from './queuePresentation';
import { agentDisplayTitle } from './presentation';
import { usePreparedConfiguration } from './preparedConfiguration';
import { messageSubmission } from './messageSubmission';
import { useFirstTextSend } from './useFirstTextSend';
import { composerInputAvailability } from './composerPresentation';
import { turnRetryAvailable } from './turnRetryAvailability';
import { WORKSPACE_READ_RECONNECTING } from './controllerError';

const SIGN_IN_AGAIN = 'subscription-sign-in';
const SIGN_IN_AGAIN_LABEL = 'Sign in to your ChatGPT subscription again';

export function ActiveAgentSurface({
  lifecycleFeedback,
  computerObservation,
  onViewDeletionStatus,
  suggestedReplies,
  lifecycleClient,
  draft,
  agentId,
  controller,
  viewPosition,
  detailsOpen,
  filesOpen,
  onFilesOpenChange,
  onDetailsOpenChange,
  onNavigate,
  viewerId,
  user,
  accessUncertain = false,
  accessAuthorized = false,
  firstSendContext,
  permissions,
  actionsBusy = false,
  onAgentAction,
}: AgentSurfaceProps & { readonly lifecycleFeedback?: ReactNode }) {
  const [steeringMode, setSteeringMode] = useState<SteeringMode>('queue');
  const headingRef = useRef<HTMLHeadingElement>(null);
  const state: AgentResourceState =
    controller.state.status === 'ready' && controller.state.detail.id !== agentId
      ? { status: 'loading' }
      : controller.state;
  useDocumentTitle(state.status === 'ready' ? agentDisplayTitle(state.detail) : undefined, 'task');
  const source = useAgentSource(agentId, state.status === 'ready' && !accessUncertain);
  const provenance = useAgentProvenance(agentId, state.status === 'ready' && !accessUncertain);
  const sandboxOwner = controller.progressObservation?.sandbox.owner;
  const originLabel = useOwnerCreationLabel(provenance, sandboxOwner, state.status === 'ready');
  const readOnly =
    accessUncertain ||
    (viewerId !== undefined && sandboxOwner !== undefined && viewerId !== sandboxOwner);
  const feedback = useMessageFeedback(agentId, viewerId, !readOnly && state.status === 'ready');

  const connection = useAgentConnectionReadiness(
    agentId,
    viewerId,
    lifecycleClient,
    !readOnly && (state.status === 'ready' || (accessAuthorized && state.status !== 'error')),
  );
  const verified = connection.verified;
  // A settled draft is local to this owner and Agent. Connection readiness
  // governs message admission, while the journal's current epoch governs the
  // display of the already saved local text.
  let draftJournal: AgentTransportJournal | undefined;
  if (
    accessAuthorized &&
    !connection.ownerMismatch &&
    !readOnly &&
    viewerId !== undefined &&
    state.status === 'ready' &&
    state.detail.id === agentId &&
    state.v2?.owner === viewerId &&
    state.v2.journal.agentId === agentId
  ) {
    try {
      if (state.v2.journal.selected() === 'responses_v2') draftJournal = state.v2.journal;
    } catch {
      // An unavailable or superseded journal cannot supply a visible draft.
    }
  }
  const connectionError = connection.error;
  const connections = useConnectionGate(!readOnly, connection.scope);
  const optimistic = useOptimisticMessages(
    connection.scope,
    !readOnly && state.status === 'ready',
    state.status === 'ready' ? state.conversation : undefined,
    state.status === 'ready' && state.v2?.owner === (viewerId ?? verified?.owner)
      ? state.v2
      : undefined,
    controller.readSuggestionRequest,
  );
  const discardPrepared = connection.discardPrepared;
  const editing = useAgentSuggestions({
    conversation: state.status === 'ready' ? state.conversation : undefined,
    viewerId: viewerId ?? verified?.owner ?? '',
    agentId,
    enabled: !readOnly && verified !== undefined,
    acceptedInput: optimistic.messages.some((message) => message.state === 'accepted'),
    provided: suggestedReplies,
    controller,
  });
  const suggestions = editing.source;
  const firstTextSend = useFirstTextSend({
    state,
    controller,
    client: lifecycleClient,
    agentId,
    owner: viewerId,
    authEpoch: firstSendContext?.authEpoch ?? 0,
    eligibleView:
      firstSendContext?.visible === true &&
      accessAuthorized &&
      !readOnly &&
      !editing.contextualInput &&
      !actionsBusy &&
      (draft === undefined || (!draft.recoveryBlocked && draft.attachments.length === 0)),
    connection,
    connections,
  });
  useLayoutEffect(() => {
    discardPrepared();
  }, [detailsOpen, discardPrepared]);
  usePreparedConfiguration(state, connection.scope, discardPrepared);

  useEffect(() => {
    if (state.status !== 'ready') headingRef.current?.focus({ preventScroll: true });
  }, [agentId, state.status]);

  if (state.status === 'idle' || state.status === 'loading')
    return <AgentOpening headingRef={headingRef} />;
  if (state.status === 'error')
    return (
      <AgentUnavailable
        message={state.message}
        retryAt={state.retryAt}
        headingRef={headingRef}
        onRetry={controller.reload}
      />
    );

  const { detail, conversation } = state;
  const inputAvailability = composerInputAvailability(
    controller.inputAvailability,
    readOnly,
    accessUncertain,
    verified !== undefined,
  );
  const messageQueue = queuePresentation(
    conversation,
    optimistic.messages,
    controller.queuedInputs,
  );
  const status = detail.status;
  const title = agentDisplayTitle(detail);
  const running = conversation.activeRunId !== undefined;
  const stopping = controller.pending === 'interrupt';
  const starting = controller.pending === 'send' && !running;
  const historyIncomplete = state.historyLoading === true || state.historyUnavailable === true;
  const controlReady = controller.controlBaselineReady ?? !historyIncomplete;
  const submitMessage = messageSubmission({
    controller,
    optimistic,
    connection,
    connections,
    agentId,
    viewerId,
    readOnly,
    running,
    steeringMode,
    detailsOpen,
    v2: state.v2,
  });
  const approvalBusy =
    controller.pending === 'approval' ||
    !controlReady ||
    !controller.inputAvailability.allowed ||
    controller.controlsUnavailable === true;
  const historyIssue = state.historyLoading
    ? 'Loading conversation activity.'
    : state.historyUnavailable
      ? 'Earlier activity could not be loaded. Retrying…'
      : undefined;
  const issue = state.workspaceUnavailable
    ? WORKSPACE_READ_RECONNECTING
    : historyIssue !== undefined
      ? (controller.streamError ?? historyIssue)
      : (connectionError ?? connections.error ?? controller.actionError);
  const observed = controller.progressObservation;
  const setupRoute = setupEnvironmentRoute(
    status,
    detail.sandbox.id,
    viewerId,
    !readOnly && permissions?.deleteSandbox === true,
    observed,
  );
  const reviewEnvironment = setupRoute ? () => onNavigate(setupRoute) : undefined;
  const currentSandbox = observed?.sandbox.id === detail.sandbox.id ? observed.sandbox : undefined;
  const headerMenu = agentHeaderMenu(status, readOnly ? undefined : permissions, {
    sandbox: currentSandbox,
    viewerId,
    stale: observed?.stale,
    busy: actionsBusy,
  });
  const signInAgain =
    !readOnly && verified?.services.some((service) => service.service === subscriptionService);
  const uploadAttachment = agentAttachmentUploader(
    agentId,
    readOnly || verified === undefined ? undefined : lifecycleClient,
  );
  return (
    <ComputerContext value={computerObservation}>
      <section className="agent-surface">
        {connections.dialog}
        <OwnerAgentHeader
          onViewDeletionStatus={onViewDeletionStatus}
          onTitleUpdated={controller.applyAgentTitle}
          detail={detail}
          readOnly={readOnly}
          actionsBusy={actionsBusy || controller.pending !== undefined || !controlReady}
          busy={
            status !== 'ready' ||
            actionsBusy ||
            controller.pending !== undefined ||
            state.historyLoading === true
          }
          filesOpen={filesOpen}
          onFilesOpenChange={accessUncertain ? undefined : onFilesOpenChange}
          title={title}
          status={status}
          activityStatus={state.activityStatus}
          turnId={
            historyIncomplete || running || accessUncertain ? undefined : conversation.latestTurnId
          }
          taskAcceptance={accessUncertain ? undefined : detail.taskAcceptance}
          headingRef={headingRef}
          viewerId={viewerId}
          branch={readOnly ? undefined : detail.worktree.branch}
          detailsOpen={detailsOpen}
          detailsUnavailable={accessUncertain}
          onDetailsOpenChange={(open) => {
            if (!accessUncertain) onDetailsOpenChange(open);
          }}
          breadcrumb={agentBreadcrumb(detail, title, onNavigate)}
          menu={[
            ...agentOriginEntries(source, provenance, originLabel),
            ...(signInAgain ? [{ id: SIGN_IN_AGAIN, label: SIGN_IN_AGAIN_LABEL }] : []),
          ]}
          onMenuSelect={(id) => {
            if (id === SIGN_IN_AGAIN)
              connections.run(
                { type: 'agent', id: agentId },
                () => {
                  controller.clearActionError();
                  controller.reload();
                },
                true,
              );
          }}
          moreActions={headerMenu}
          onReviewEnvironment={reviewEnvironment}
          onMoreAction={agentHeaderActionHandler(
            agentId,
            readOnly,
            onAgentAction,
            () => onNavigate({ kind: 'sandbox', sandboxId: detail.sandbox.id }),
            currentSandbox?.id,
          )}
        />

        <div className="agent-surface__alerts">
          <AgentAccessRequests
            key={`access:${agentId}`}
            agentId={agentId}
            owner={viewerId}
            granted={detail.reach}
            sandboxStatus={detail.sandbox.status}
            running={running}
            eventRevision={conversation.agentCursor ?? conversation.cursor}
            enabled={!readOnly && viewerId !== undefined}
          />
          <AgentSurfaceIssue
            issue={issue}
            unconfirmed={controller.pendingMessage !== undefined || controller.pendingControl}
            historyIncomplete={historyIncomplete || state.workspaceUnavailable === true}
            onRetry={connectionError && !readOnly ? connection.retry : undefined}
            onDismiss={() => {
              connection.clearError();
              connections.clearError();
              controller.clearActionError();
            }}
          />

          {lifecycleFeedback}
        </div>

        <div className="agent-surface__conversation">
          <OwnerMessageStream
            taskAcceptance={accessUncertain ? undefined : detail.taskAcceptance}
            onTaskDeliveryConfirmed={controller.refreshDetail}
            client={lifecycleClient}
            agentId={agentId}
            viewerId={viewerId}
            authorized={!accessUncertain && accessAuthorized}
            feedback={feedback}
            user={user}
            viewPosition={viewPosition}
            conversation={conversation}
            stopping={stopping}
            agentStatus={status}
            optimisticMessages={messageQueue.messages}
            arrivalsEnabled={!accessUncertain}
            approvalBusy={approvalBusy}
            approvalReadOnly={readOnly}
            onApproval={(id, decision) => void controller.approve(id, decision)}
            onReviewRetry={
              turnRetryAvailable(
                controller,
                !readOnly && verified !== undefined,
                draft,
                actionsBusy,
              )
                ? editing.restore
                : undefined
            }
          >
            <AgentCommandNotices
              key={JSON.stringify([agentId, viewerId])}
              controller={controller}
              unavailable={readOnly || !controlReady}
              starting={status === 'starting'}
            />
            <AgentStatusPanel
              detail={detail}
              events={state.sandboxEvents}
              observation={observed}
              running={running && state.activityStatus !== 'waiting_approval'}
              historyIncomplete={historyIncomplete}
            />
            {suggestions && !readOnly && verified && !historyIncomplete ? (
              <SuggestedReplies
                onEdit={editing.admissionAllowed ? editing.edit : undefined}
                key={suggestions.id}
                suggestions={suggestions}
                controller={controller}
                disabled={
                  !editing.admissionAllowed ||
                  connections.pending ||
                  !controller.inputAvailability.allowed ||
                  controller.pending !== undefined ||
                  controller.pendingMessage !== undefined ||
                  controller.pendingControl === true
                }
                onSend={(text, accepted, refused) =>
                  submitMessage(text, accepted, suggestions.scope, refused)
                }
              />
            ) : null}
            <AgentSandboxStatus
              observation={controller.progressObservation}
              onNavigate={onNavigate}
              onReviewEnvironment={reviewEnvironment}
            />
          </OwnerMessageStream>
        </div>
        <div className="agent-surface__input">
          <AgentInputQueue
            controller={controller}
            queue={messageQueue}
            readOnly={readOnly}
            controlReady={controlReady}
            verified={verified !== undefined}
            draftJournal={draftJournal}
            draftIdentityCurrent={connection.ownerIdentityCurrent}
            draftBlocked={draft?.recoveryBlocked}
            onRestore={editing.restore}
            uploadAttachment={uploadAttachment}
            user={user}
          />
          <Composer
            onRestart={readOnly || !detail.restartAvailable ? undefined : controller.restart}
            editorRef={editing.editorRef}
            suggestionScope={
              !readOnly && verified && !historyIncomplete ? suggestions?.id : undefined
            }
            settings={
              readOnly ? undefined : (
                <SessionSettings
                  usage={conversation?.usage}
                  key={`settings:${viewerId}:${agentId}`}
                  agentId={agentId}
                  viewerId={viewerId}
                  active={!accessUncertain}
                  runtimeReady={modelCatalogReady(detail, observed, state.sandboxEvents)}
                  busy={status !== 'ready' || controller.pending !== undefined || historyIncomplete}
                />
              )
            }
            steeringMode={steeringMode}
            onSteeringModeChange={state.v2 === undefined ? undefined : setSteeringMode}
            draft={draft}
            availableCommands={readOnly ? undefined : conversation.availableCommands}
            key={`composer:${viewerId}:${agentId}`}
            quickMessages={
              source.status === 'ready' && source.value.access !== 'shared'
                ? source.value.trigger?.quick_messages
                : undefined
            }
            status={status}
            pausedNotice={
              conversation.livePausedNotice === undefined
                ? detail.pausedNotice
                : (conversation.livePausedNotice ?? undefined)
            }
            running={running}
            queueWhileRunning={state.v2 !== undefined}
            queueWhileStarting={state.v2 !== undefined}
            starting={starting}
            controlsUnavailable={controller.controlsUnavailable || !controlReady}
            responseControls={
              accessUncertain
                ? 'checking-access'
                : readOnly
                  ? 'denied'
                  : !controlReady
                    ? 'checking'
                    : controller.responseControls
            }
            requestPending={
              controller.pendingMessage !== undefined || controller.pendingControl === true
            }
            acceptedMessage={controller.acceptedMessage}
            acceptedSteeringMessage={controller.acceptedSteeringMessage}
            acceptedInterruptionMessage={controller.acceptedInterruptionMessage}
            uploadAttachment={uploadAttachment}
            firstTextSend={firstTextSend}
            inputAvailability={
              readOnly || !verified
                ? inputAvailability
                : editing.inputAvailability(inputAvailability)
            }
            busy={
              connections.pending ||
              (controller.pending !== undefined && controller.pending !== 'send')
            }
            stopping={stopping}
            onSend={(content, accepted, brokerReport) =>
              brokerReport
                ? submitMessage(content, accepted, undefined, undefined, brokerReport)
                : editing.send(submitMessage, content, accepted)
            }
            onStop={controller.interrupt}
          />
        </div>
        {verified === undefined && connection.needed && !readOnly && !firstTextSend.pending ? (
          <AgentConnectionSetup
            agentId={agentId}
            owner={viewerId}
            explicit={firstTextSend.available}
            gate={connections}
            onReady={connection.accept}
            onCancel={() => onNavigate({ kind: 'home' })}
          />
        ) : null}
      </section>
    </ComputerContext>
  );
}
