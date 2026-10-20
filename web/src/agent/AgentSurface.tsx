import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { useConnectionGate } from '../connections/useConnectionGate';
import {
  connectionClient,
  subscriptionService,
  type ConnectionReport,
} from '../connections/client';
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
import { Resources } from './Resources';
import { BrandLoading } from '../landing/LandingCharacter';
import { LifecycleProgress } from '../sandbox/LifecycleProgress';
import { AgentSourceLabel, useAgentSource } from './source';
import { useAgentProvenance } from './provenance';
import { QueuedMessageNotice } from './QueuedMessageNotice';
import { AgentAccessRequests } from './AccessRequestDialog';
import { SessionSettings } from './SessionSettings';

interface AgentSurfaceProps {
  readonly draft?: AgentDraft;
  readonly viewerId?: string;
  readonly agentId: string;
  readonly controller: AgentController;
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
  draft,
  agentId,
  controller,
  detailsOpen,
  onDetailsOpenChange,
  onNavigate,
  viewerId,
}: AgentSurfaceProps) {
  const headingRef = useRef<HTMLHeadingElement>(null);
  const [verified, setVerified] = useState<ConnectionReport>();
  const [connectionNeeded, setConnectionNeeded] = useState(false);
  const [connectionError, setConnectionError] = useState<string>();
  const [connectionRetry, setConnectionRetry] = useState(0);
  const client = useCmaClient();
  const connections = useConnectionGate();
  const state: AgentResourceState =
    controller.state.status === 'ready' && controller.state.detail.id !== agentId
      ? { status: 'loading' }
      : controller.state;
  const documentTitle =
    state.status === 'ready' ? `${state.detail.title ?? 'Untitled agent'} · CMA` : undefined;
  const source = useAgentSource(agentId, state.status === 'ready');
  const provenance = useAgentProvenance(agentId, state.status === 'ready');
  const sandboxOwner = controller.progressObservation?.sandbox.owner;
  const readOnly =
    viewerId !== undefined && sandboxOwner !== undefined && viewerId !== sandboxOwner;

  useEffect(() => {
    if (state.status !== 'ready' || readOnly) return;
    const abort = new AbortController();
    void connectionClient
      .check({ type: 'agent', id: agentId }, undefined, abort.signal)
      .then((report) => {
        if (abort.signal.aborted) return;
        if (viewerId !== undefined && report.owner !== viewerId) {
          setConnectionError('Your CMA account changed. Open the Agent again to continue.');
          return;
        }
        if (report.ready) {
          setVerified(report);
          setConnectionError(undefined);
        } else if (
          report.services.some((row) => row.state === 'missing' || row.state === 'inactive')
        )
          setConnectionNeeded(true);
        else setConnectionError('The connections could not be verified. Please try again.');
      })
      .catch((error: unknown) => {
        if (!abort.signal.aborted)
          setConnectionError(
            error instanceof Error ? error.message : 'The connections could not be verified.',
          );
      });
    return () => abort.abort();
  }, [agentId, state.status, readOnly, viewerId, connectionRetry]);

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
  const status = detail.status;
  const title = detail.title?.trim() || 'Untitled agent';
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

  return (
    <section className="agent-surface">
      {connections.dialog}
      <AgentAccessRequests
        key={`access:${agentId}`}
        agentId={agentId}
        owner={viewerId}
        granted={detail.reach}
        enabled={!readOnly && viewerId !== undefined}
      />
      <AgentHeader
        title={title}
        status={status}
        headingRef={headingRef}
        detailsOpen={detailsOpen}
        onDetailsOpenChange={onDetailsOpenChange}
        lineage={
          <>
            <SandboxLink
              id={detail.sandbox.id}
              name={detail.sandbox.name}
              onNavigate={onNavigate}
            />
            <span aria-hidden="true">/</span>
            <span>{detail.workspace.name}</span>
            <span aria-hidden="true">/</span>
            <span>Agent</span>
          </>
        }
      >
        <AgentSourceLabel source={source} provenance={provenance} />
        {verified?.services.some((service) => service.service === subscriptionService) ? (
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
                  setConnectionError(undefined);
                  connections.clearError();
                  controller.clearActionError();
                }}
              />
            )}
            {connectionError ? (
              <Button size="sm" onClick={() => setConnectionRetry((value) => value + 1)}>
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
            disabled={controller.pending !== undefined || historyIncomplete}
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

      <QueuedMessageNotice control={controller.queuedMessage} readOnly={readOnly} />

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
          conversation={conversation}
          approvalBusy={approvalBusy}
          onApproval={(id, decision) => void controller.approve(id, decision)}
        >
          {controller.progressObservation === undefined ? null : (
            <LifecycleProgress {...controller.progressObservation} />
          )}
        </MessageStream>
      </div>
      <div className="agent-surface__input">
        <ActiveThinking key={agentId} conversation={conversation} />
        <Composer
          draft={draft}
          key={`composer:${agentId}`}
          quickMessages={
            source.status === 'ready' && source.value.access !== 'shared'
              ? source.value.trigger?.quick_messages
              : undefined
          }
          settings={
            client.readAgentSettings && client.updateAgentSettings ? (
              <SessionSettings
                key={`inline-settings:${agentId}`}
                agentId={agentId}
                active
                inline
                busy={status !== 'ready' || approvalBusy}
              />
            ) : undefined
          }
          status={status}
          pausedNotice={pausedNotice}
          running={running}
          queueWhileRunning={state.v2 !== undefined}
          starting={starting}
          controlsUnavailable={controller.controlsUnavailable}
          requestPending={controller.pendingMessage !== undefined}
          acceptedMessage={controller.acceptedMessage}
          inputAvailability={
            readOnly
              ? {
                  ...controller.inputAvailability,
                  allowed: false,
                  notice: 'Only the owner can send messages to this agent.',
                }
              : verified === undefined
                ? {
                    ...controller.inputAvailability,
                    allowed: false,
                    notice: 'Verify the required connections to continue.',
                  }
                : controller.inputAvailability
          }
          busy={
            connections.pending ||
            historyIncomplete ||
            (controller.pending !== undefined && controller.pending !== 'send')
          }
          onSend={async (content, onAccepted) => {
            if (verified === undefined) return;
            await connections.checkAndRun(
              { type: 'agent', id: agentId },
              viewerId ?? verified.owner,
              () => void controller.send(content, onAccepted),
            );
          }}
          onStop={controller.interrupt}
        />
      </div>
      {verified === undefined && connectionNeeded && !readOnly ? (
        <ConnectionWizard
          target={{ type: 'agent', id: agentId }}
          presentation="dialog"
          expectedOwner={viewerId}
          onReady={(report) => {
            setVerified(report);
            setConnectionNeeded(false);
          }}
          onCancel={() => onNavigate({ kind: 'home' })}
        />
      ) : null}
      <Resources
        key={agentId}
        agentId={agentId}
        sandboxId={detail.sandbox.id}
        sandboxProfile={detail.sandbox.profile}
        availability={controller.resourceAvailability}
        usage={conversation.usage}
      />
    </section>
  );
}
