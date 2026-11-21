import { ComputerContext } from './environment/context';
import { SuggestedReplies } from './SuggestedReplies';
import type { SuggestedReplySet } from './suggestions';
import { useAgentSuggestions } from './useAgentSuggestions';
import { Presence } from '../ui/Presence';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { useConnectionGate } from '../connections/useConnectionGate';
import { subscriptionService } from '../connections/client';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import type { AgentController, AgentResourceState } from './controller';
import { Composer } from './Composer';
import { SessionSettings } from './SessionSettings';
import { modelCatalogReady } from './modelReadiness';
import { useMessageFeedback } from './useMessageFeedback';
import type { AgentDraft } from './draft';
import { OwnerMessageStream } from './OwnerMessageStream';
import './Agent.css';
import { OwnerAgentHeader } from './OwnerAgentHeader';
import { Alert } from '../ui/Alert';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { agentBreadcrumb, agentHeaderMenu, agentHeaderActionHandler } from './headerActions';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { ProvisioningProgress } from '../sandbox/ProvisioningProgress';
import { agentProgress } from '../sandbox/progress';
import type { SteeringMode } from './SteeringMode';
import { AgentOpening, AgentUnavailable, AgentCommandNotices } from './AgentSurfaceStates';
import { AgentSandboxStatus } from './AgentSandboxStatus';
import { setupEnvironmentRoute } from './setupEnvironment';
import { agentOriginEntries, useAgentSource } from './source';
import { useAgentProvenance, useOwnerCreationLabel } from './provenance';
import { AgentInputQueue } from './AgentInputQueue';
import { agentAttachmentUploader } from './attachmentUpload';
import { AgentAccessRequests } from './AccessRequestDialog';
import type { CmaWebClient } from '../api/client';
import { AgentLifecycleFollowup } from '../create/AgentLifecycleFollowup';
import { useAgentConnectionReadiness } from './connectionReadiness';
import { useOptimisticMessages } from './optimisticMessages';
import { queuePresentation } from './queuePresentation';
import type { AgentViewPosition } from './viewPosition';
import type { MessageUser } from './MessageIdentity';
import { agentDisplayTitle } from './presentation';
import { usePreparedConfiguration } from './preparedConfiguration';
import { messageSubmission } from './messageSubmission';
import type { AgentComputerObservation } from './environment/observation';

const SIGN_IN_AGAIN = 'subscription-sign-in';
const SIGN_IN_AGAIN_LABEL = 'Sign in to your ChatGPT subscription again';

interface AgentSurfaceProps {
  readonly computerObservation?: AgentComputerObservation;
  readonly onViewDeletionStatus?: () => void;
  readonly suggestedReplies?: SuggestedReplySet | null;
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
  readonly filesOpen?: boolean;
  readonly onFilesOpenChange?: (open: boolean) => void;
  readonly detailsOpen: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly onNavigate: (route: AppRoute) => void;
  /** What the viewer may do to this Agent from its header. */
  readonly permissions?: TreePermissions;
  /** Another resource action is still being confirmed. */
  readonly actionsBusy?: boolean;
  readonly onAgentAction?: (action: TreeAction) => void;
}

export function AgentSurface({
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
  permissions,
  actionsBusy = false,
  onAgentAction,
}: AgentSurfaceProps) {
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
  const issue = state.historyLoading
    ? 'Loading conversation activity.'
    : state.historyUnavailable
      ? 'Earlier activity could not be loaded. Retrying…'
      : (connectionError ?? connections.error ?? controller.actionError);
  const preparation = agentProgress({ ...detail, status }, state.sandboxEvents ?? []);
  const observed = controller.progressObservation;
  const setupRoute = setupEnvironmentRoute(
    status,
    detail.sandbox.id,
    viewerId,
    !readOnly && permissions?.deleteSandbox === true,
    observed,
  );
  const reviewEnvironment = setupRoute ? () => onNavigate(setupRoute) : undefined;
  const headerMenu = agentHeaderMenu(status, readOnly ? undefined : permissions, {
    sandbox: observed?.sandbox.id === detail.sandbox.id ? observed.sandbox : undefined,
    viewerId,
    stale: observed?.stale,
    busy: actionsBusy,
  });
  // A subscription Agent can be signed in again from the header menu.
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
          onMoreAction={agentHeaderActionHandler(agentId, readOnly, onAgentAction, () =>
            onNavigate({ kind: 'sandbox', sandboxId: detail.sandbox.id }),
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
          <Presence>
            {issue === undefined ? null : historyIncomplete ? (
              <p className="agent-surface__history-notice" role="status">
                {issue}
              </p>
            ) : (
              <Alert
                tone={historyIncomplete ? 'info' : 'error'}
                actions={
                  <>
                    {connectionError && !readOnly ? (
                      <Button size="sm" onClick={connection.retry}>
                        Retry connections
                      </Button>
                    ) : null}
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
                  </>
                }
              >
                {issue}
              </Alert>
            )}
          </Presence>

          <AgentCommandNotices
            controller={controller}
            unavailable={readOnly || !controlReady}
            starting={status === 'starting'}
          />

          {!readOnly && lifecycleClient !== undefined && viewerId !== undefined ? (
            <AgentLifecycleFollowup
              client={lifecycleClient}
              viewerId={viewerId}
              agentId={agentId}
            />
          ) : null}
        </div>

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
          <OwnerMessageStream
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
          >
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
            draftBlocked={draft?.recoveryBlocked}
            onRestore={editing.restore}
            uploadAttachment={uploadAttachment}
            user={user}
          />
          <Composer
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
            pausedNotice={pausedNotice}
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
                  : editing.inputAvailability(controller.inputAvailability)
            }
            busy={
              connections.pending ||
              (controller.pending !== undefined && controller.pending !== 'send')
            }
            stopping={stopping}
            onSend={(content, accepted) => editing.send(submitMessage, content, accepted)}
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
    </ComputerContext>
  );
}
