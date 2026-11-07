import { Resources } from './Resources';
import type { SandboxResourceAvailability } from './sandboxInput';
import { useEffect, useRef } from 'react';
import type { AgentDetail } from '../api/types';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import type { AgentCommand } from './controller';
import './Details.css';
import { DetailsServiceAccess } from './DetailsServiceAccess';
import { openSandboxTab } from '../sandbox/sandboxTabRoute';
import { SkillsSettings } from './SkillsSettings';
import { AgentFiles } from './files/AgentFiles';
import { AgentShareLinks } from '../resources/AgentShareLinks';

interface AgentDetailsProps {
  readonly availability?: SandboxResourceAvailability;
  readonly detail?: AgentDetail;
  readonly pending?: AgentCommand;
  readonly historyLoading?: boolean;
  readonly open?: boolean;
  readonly onClose?: () => void;
  /** Opens the Agent's Sandbox page; the drawer picks which tab. */
  readonly onOpenSandbox?: () => void;
}

export function AgentDetails({
  detail,
  availability = { state: 'checking', notice: 'Checking Sandbox availability…' },
  pending,
  historyLoading = false,
  open = false,
  onClose,
  onOpenSandbox,
}: AgentDetailsProps) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement;
    heading.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [open]);
  if (detail === undefined) {
    return <aside className="agent-details" aria-label="Agent details" aria-busy="true" />;
  }

  return (
    <aside className="agent-details" aria-label="Agent details">
      <header className="agent-details__header">
        <h1 ref={heading} tabIndex={-1}>
          Details
        </h1>
        {onClose === undefined ? null : (
          <Button
            size="sm"
            icon={<IconCloseOutline16 />}
            aria-label="Close Agent details"
            title="Close Agent details"
            onClick={onClose}
          />
        )}
      </header>
      <div className="agent-details__scroll">
        {detail.workspace.source?.type === 'repository_snapshot' ? (
          <section className="agent-details__band" aria-label="Repository context">
            <h2>Repository context</h2>
            <dl>
              <div className="agent-details__fact">
                <dt>Repository</dt>
                <dd>{detail.workspace.source.url}</dd>
              </div>
              <div className="agent-details__fact">
                <dt>Requested ref</dt>
                <dd>{detail.workspace.source.reference ?? 'Default branch'}</dd>
              </div>
              <div className="agent-details__fact">
                <dt>Source commit</dt>
                <dd>
                  <code>{detail.workspace.source.source_commit ?? 'Not prepared yet'}</code>
                </dd>
              </div>
              <div className="agent-details__fact">
                <dt>Checkout commit</dt>
                <dd>
                  <code>{detail.workspace.resolvedCommit ?? 'Not prepared yet'}</code>
                </dd>
              </div>
            </dl>
          </section>
        ) : null}
        {open ? (
          <section className="agent-details__band" aria-label="Sandbox machine">
            <h2>Sandbox machine</h2>
            <dl>
              <div className="agent-details__fact">
                <dt>Sandbox id</dt>
                <dd>
                  <code>{detail.sandbox.id}</code>
                </dd>
              </div>
            </dl>
            <Resources
              agentId={detail.id}
              sandboxId={detail.sandbox.id}
              sandboxProfile={detail.sandbox.profile}
              availability={availability}
            />
            <h3>Agent key scope</h3>
            {detail.reach.length ? (
              <ul>
                {[...new Set(detail.reach)].map((scope) => (
                  <li key={scope}>{scope}</li>
                ))}
              </ul>
            ) : (
              <p>No services are enabled.</p>
            )}
          </section>
        ) : null}
        <AgentFiles key={`files:${detail.id}`} agentId={detail.id} active={open} />
        <AgentShareLinks key={`sharing:${detail.id}`} agentId={detail.id} active={open} />
        <DetailsServiceAccess detail={detail} onOpenSandbox={onOpenSandbox} />
        <SkillsSettings
          key={`skills:${detail.id}`}
          agentId={detail.id}
          active={open}
          busy={detail.status !== 'ready' || pending !== undefined || historyLoading}
        />
        {onOpenSandbox === undefined ? null : (
          <button
            type="button"
            className="agent-details__link"
            onClick={() => openSandboxTab(onOpenSandbox, detail.sandbox.id, 'activity')}
          >
            Sandbox operation progress
          </button>
        )}
      </div>
    </aside>
  );
}
