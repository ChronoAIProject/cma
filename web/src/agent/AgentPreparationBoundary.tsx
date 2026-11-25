import { useEffect, useRef } from 'react';
import {
  AgentLifecycleFollowup,
  type LifecycleFollowupView,
} from '../create/AgentLifecycleFollowup';
import { lifecycleFailureMessage } from '../create/lifecycleFailure';
import { PreparationView } from '../preparation/PreparationView';
import { PreparationDraft } from '../preparation/PreparationDraft';
import { PreparationDraftRecovery } from '../preparation/PreparationDraftRecovery';
import { usePreparationDraft } from '../preparation/usePreparationDraft';
import { derivePreparationView } from '../preparation/preparationPresentation';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { Button } from '../ui/Button';
import { ActiveAgentSurface } from './ActiveAgentSurface';
import type { AgentSurfaceProps } from './AgentSurfaceProps';
import { setupEnvironmentRoute, SETUP_ENVIRONMENT_NOTICE } from './setupEnvironment';
import { AgentCommandNotices } from './AgentSurfaceStates';
import { ProvisioningProgress } from '../sandbox/ProvisioningProgress';
import { agentProgress } from '../sandbox/progress';

export function AgentPreparationBoundary(props: AgentSurfaceProps) {
  const { lifecycleClient, viewerId, agentId } = props;
  if (!lifecycleClient || !viewerId) return <ActiveAgentSurface {...props} />;
  return (
    <AgentLifecycleFollowup
      client={lifecycleClient}
      viewerId={viewerId}
      agentId={agentId}
      enabled={!props.accessUncertain}
      render={(followup) => (
        <PreparationHost
          {...props}
          lifecycleClient={lifecycleClient}
          viewerId={viewerId}
          followup={followup}
        />
      )}
    />
  );
}

function PreparationHost(
  props: AgentSurfaceProps & {
    readonly lifecycleClient: NonNullable<AgentSurfaceProps['lifecycleClient']>;
    readonly viewerId: string;
    readonly followup: LifecycleFollowupView;
  },
) {
  const { lifecycleClient: client, viewerId, agentId, controller, followup, draft } = props;
  const { lifecycle, context } = followup;
  const { intent, operation } = lifecycle ?? {};
  const pendingDraft = usePreparationDraft(
    client,
    intent
      ? {
          owner: viewerId,
          intentKey: intent.key,
          ...context,
        }
      : undefined,
  );
  const ready =
    controller.state.status === 'ready' && controller.state.detail.id === agentId
      ? controller.state
      : undefined;
  const detail = ready?.detail;
  const authorized = props.accessAuthorized === true && !props.accessUncertain;
  const matched =
    intent?.agentId === agentId &&
    operation?.id === intent?.receipt?.id &&
    operation?.agent_id === agentId;
  const first = detail?.firstMessage;
  const started = Boolean(
    ready?.conversation.latestTurnId ||
    ready?.conversation.activeRunId ||
    ready?.conversation.messages.length ||
    (first?.purpose === 'task' &&
      (first.state === 'execution_observed' || first.state === 'completed')),
  );
  const hasRequest = intent?.body.initial_message !== undefined || first?.purpose === 'task';
  const preparing =
    authorized &&
    detail !== undefined &&
    hasRequest &&
    !started &&
    (intent !== undefined || detail.status === 'starting' || first?.purpose === 'task');
  useDocumentTitle(
    preparing ? detail?.ownerTitle || detail?.title || 'New agent' : undefined,
    preparing ? 'task' : 'inherit',
  );
  const clearAttempt = useRef<typeof operation>(undefined);
  useEffect(() => {
    if (!authorized || !detail || !matched || !operation || !lifecycle) return;
    const bound = pendingDraft.bind({
      operationId: operation.id,
      agentId,
      worktreeId: detail.worktree.id,
      workspaceId: detail.workspace.id,
      sandboxId: detail.sandbox.id,
    });
    if (
      bound &&
      operation.status === 'succeeded' &&
      !preparing &&
      clearAttempt.current !== operation
    ) {
      clearAttempt.current = operation;
      lifecycle.clear();
    }
  }, [authorized, detail, matched, operation, lifecycle, pendingDraft, preparing, agentId]);
  if (!preparing || !detail)
    return (
      <ActiveAgentSurface
        {...props}
        lifecycleFeedback={
          <>
            {detail && authorized && !intent ? (
              <PreparationDraftRecovery
                client={client}
                viewerId={viewerId}
                detail={detail}
                draft={draft}
                authorized={
                  !controller.pendingMessage && !controller.pendingControl && !controller.pending
                }
                revision={pendingDraft.record?.revision}
                epoch={props.firstSendContext?.authEpoch}
              />
            ) : null}
            {intent && (pendingDraft.text || pendingDraft.error) ? (
              <PreparationDraft
                draft={pendingDraft}
                unavailableReason="Your original setup is still being checked. Keep or copy this follow-up while its saved binding is verified."
              />
            ) : null}
            {followup.feedback}
            {!hasRequest ? followup.details : null}
          </>
        }
      />
    );
  const review = setupEnvironmentRoute(
    detail.status,
    detail.sandbox.id,
    viewerId,
    props.permissions?.deleteSandbox === true,
    controller.progressObservation,
  );
  const failure = lifecycleFailureMessage(lifecycle?.failure, {
    label: 'Retry setup status',
    kind: 'status',
  });
  const observation = controller.progressObservation;
  const confirmedSandbox =
    observation?.sandbox.id === detail.sandbox.id && !observation.stale
      ? observation.sandbox
      : undefined;
  const sandboxStatus = confirmedSandbox?.status ?? detail.sandbox.status;
  const setupFailure = detail.status === 'failed' || sandboxStatus === 'failed';
  const failureDetail = detail.failure ?? confirmedSandbox?.failure;
  const model = derivePreparationView({
    receipt: intent?.receipt,
    operation: matched ? operation : undefined,
    readError: failure,
    requestAccepted: first?.purpose === 'task',
    setupFailure: setupFailure
      ? [
          failureDetail?.detail ?? 'The Agent could not finish setup.',
          failureDetail?.remediation ?? 'Review its environment before trying again.',
        ].join(' ')
      : undefined,
    canonicalTitle: detail.ownerTitle || detail.title,
    initialTask: first?.purpose === 'task' && first.state === 'queued' ? 'not-started' : 'unknown',
    environmentReady: sandboxStatus === 'active' && detail.status !== 'starting',
  });
  return (
    <section className="agent-surface agent-preparation">
      <PreparationView
        model={model}
        message={intent?.body.initial_message}
        details={
          followup.details ?? (
            <ProvisioningProgress
              title="Recorded setup progress"
              steps={agentProgress(detail, ready?.sandboxEvents ?? [])}
              failure={setupFailure ? undefined : detail.failure}
            />
          )
        }
        notices={
          <>
            {followup.feedback}
            <AgentCommandNotices
              controller={controller}
              unavailable={!authorized}
              starting={detail.status === 'starting'}
            />
            {ready?.historyUnavailable ? (
              <p role="alert">
                Conversation updates interrupted. Earlier activity is not confirmed.
              </p>
            ) : null}
            {controller.actionError ? <p role="alert">{controller.actionError}</p> : null}
            {controller.progressObservation?.stale ? (
              <p role="status">
                Environment updates interrupted. Showing the last confirmed observation.
              </p>
            ) : null}
          </>
        }
        actions={
          review ? (
            <>
              <p>{SETUP_ENVIRONMENT_NOTICE}</p>
              <Button onClick={() => props.onNavigate(review)}>Review environment</Button>
            </>
          ) : undefined
        }
        draft={
          intent ? (
            <PreparationDraft draft={pendingDraft} />
          ) : draft ? (
            <PreparationDraft
              draft={{
                text: draft.content,
                change: draft.setContent,
                error: draft.error,
                retryStorage: draft.retryStorage,
              }}
            />
          ) : undefined
        }
      />
    </section>
  );
}
