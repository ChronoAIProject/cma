import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { useConnectionGate } from '../connections/useConnectionGate';
import { subscriptionService, type ConnectionReport } from '../connections/client';
import type { AgentStatus } from '../api/types';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { Button } from '../ui/Button';
import { StateDot, type StateDotState } from '../ui/StateDot';
import { IconCloseOutline16, IconInspectOutline12 } from '../ui/icons';
import type { AgentController, AgentResourceState } from './controller';
import { Composer } from './Composer';
import { MessageStream } from './MessageStream';
import './Agent.css';
import { statusLabel } from '../vocabulary/status';
import { ProvisioningProgress } from '../sandbox/ProvisioningProgress';
import { agentProgress } from '../sandbox/progress';
import { Resources } from './Resources';
import { BrandLoading } from '../landing/LandingCharacter';
import { LifecycleProgress } from '../sandbox/LifecycleProgress';
import { AgentSourceLabel, useAgentSource } from './source';

interface AgentSurfaceProps {
  readonly viewerId?: string;
  readonly agentId: string;
  readonly controller: AgentController;
  readonly detailsOpen: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly onNavigate: (route: AppRoute) => void;
}

function statusDot(status: AgentStatus): StateDotState {
  if (status === 'failed' || status === 'deleted') return 'error';
  if (status === 'paused' || status === 'waiting_approval') return 'warning';
  if (status === 'working' || status === 'starting') return 'ongoing';
  return 'done';
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
  agentId,
  controller,
  detailsOpen,
  onDetailsOpenChange,
  onNavigate,
  viewerId,
}: AgentSurfaceProps) {
  const headingRef = useRef<HTMLHeadingElement>(null);
  const [verified, setVerified] = useState<ConnectionReport>();
  const connections = useConnectionGate();
  const state: AgentResourceState =
    controller.state.status === 'ready' && controller.state.detail.id !== agentId
      ? { status: 'loading' }
      : controller.state;
  const documentTitle =
    state.status === 'ready' ? `${state.detail.title ?? 'Untitled agent'} · CMA` : undefined;
  const source = useAgentSource(agentId, state.status === 'ready');
  const sandboxOwner = controller.progressObservation?.sandbox.owner;
  const readOnly =
    viewerId !== undefined && sandboxOwner !== undefined && viewerId !== sandboxOwner;

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
    controller.pending === 'approval' || historyIncomplete || !controller.inputAvailability.allowed;
  const issue = state.historyLoading
    ? 'Loading conversation activity.'
    : state.historyUnavailable
      ? 'Conversation activity is unavailable. Reconnecting.'
      : controller.actionError;
  const preparation = agentProgress({ ...detail, status }, state.sandboxEvents ?? []);

  return (
    <section className="agent-surface">
      {connections.dialog}
      <header className="agent-header">
        <nav className="agent-header__lineage" aria-label="Agent location">
          <SandboxLink id={detail.sandbox.id} name={detail.sandbox.name} onNavigate={onNavigate} />
          <span aria-hidden="true">/</span>
          <span>{detail.workspace.name}</span>
          <span aria-hidden="true">/</span>
          <span>Agent</span>
        </nav>
        <div className="agent-header__title">
          <div>
            <h1 ref={headingRef} tabIndex={-1}>
              {title}
            </h1>
            <span className="agent-header__status">
              <StateDot state={statusDot(status)} size={9} />
              {statusLabel(status)}
            </span>
          </div>
          <Button
            size="sm"
            variant="toolbar"
            icon={<IconInspectOutline12 />}
            aria-label={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
            title={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
            aria-pressed={detailsOpen}
            onClick={() => onDetailsOpenChange(!detailsOpen)}
          />
        </div>
        <AgentSourceLabel source={source} />
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
      </header>

      <Presence>
        {issue === undefined ? null : (
          <div className="agent-action-error" role={historyIncomplete ? 'status' : 'alert'}>
            <span>{issue}</span>
            {historyIncomplete ? null : (
              <Button
                size="sm"
                icon={<IconCloseOutline16 />}
                aria-label="Dismiss error"
                onClick={controller.clearActionError}
              />
            )}
          </div>
        )}
      </Presence>

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
      <Composer
        key={`composer:${agentId}`}
        quickMessages={source.status === 'ready' ? source.value.trigger?.quick_messages : undefined}
        status={status}
        pausedNotice={pausedNotice}
        running={running}
        starting={starting}
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
          historyIncomplete || (controller.pending !== undefined && controller.pending !== 'send')
        }
        onSend={async (content, onAccepted) => {
          connections.run(
            { type: 'agent', id: agentId },
            () => void controller.send(content, onAccepted),
          );
        }}
        onStop={controller.interrupt}
      />
      {verified === undefined && !readOnly ? (
        <ConnectionWizard
          target={{ type: 'agent', id: agentId }}
          onReady={setVerified}
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
