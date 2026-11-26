import { useEffect, useRef } from 'react';
import { useCmaClient } from '../api/context';
import { useAgentAccess } from './agentAccess';
import { ResourceAccessDetails } from './ResourceAccessSummary';
import { ChannelUsage } from './ChannelUsage';
import { ArtifactButton } from './ArtifactButton';
import type { ArtifactPublicationScope } from './artifactRecovery';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import { statusLabel } from '../vocabulary/status';
import '../agent/Details.css';
import { AgentFiles } from '../agent/files/AgentFiles';
import { creationLabel, useAgentProvenance } from '../agent/provenance';

/** Only the accessible projection is read; a narrow Agent share grants no ancestry. */
export function SharedAgentDetails({
  agent,
  open,
  onClose,
  onAccessChanged,
  publicationScope,
  workingDirectory,
}: {
  readonly agent: string;
  readonly workingDirectory?: string;
  readonly open: boolean;
  readonly onClose: () => void;
  readonly onAccessChanged?: (downloadMessage?: string) => void;
  readonly publicationScope?: ArtifactPublicationScope;
}) {
  const client = useCmaClient().resourceViews;
  const current = useAgentAccess(client, open ? agent : undefined);
  const ready = current.state.status === 'ready' ? current.state.value : undefined;
  const detail = ready?.detail.access === 'shared' ? ready.detail : undefined;
  const provenance = useAgentProvenance(agent, open && detail !== undefined);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement;
    heading.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [open]);

  if (!open) return null;
  return (
    <aside className="agent-details" aria-label="Agent details">
      <header className="agent-details__header">
        <h1 ref={heading} tabIndex={-1}>
          Details
        </h1>
        {detail === undefined ? null : <span>{statusLabel(detail.status)}</span>}
        <Button
          size="sm"
          icon={<IconCloseOutline16 />}
          aria-label="Close Agent details"
          title="Close Agent details"
          onClick={onClose}
        />
      </header>
      <div className="agent-details__scroll">
        {current.state.status === 'loading' ? <p role="status">Checking Agent access</p> : null}
        {current.state.status === 'error' ? <p role="alert">{current.state.message}</p> : null}
        {client === undefined || detail === undefined || ready?.access === undefined ? null : (
          <>
            {ready.access.capabilities.includes('view_files') ? (
              <AgentFiles
                key={`files:${agent}`}
                agentId={agent}
                workingDirectory={workingDirectory}
                active={open}
                readOnly={!ready.access.capabilities.includes('operate_agent')}
                onAccessChanged={onAccessChanged}
              />
            ) : null}
            <section className="agent-details__band">
              <h2>Organization access</h2>
              <ResourceAccessDetails client={client} access={ready.access} />
              <p>Your messages are attributed to you.</p>
            </section>
            <section className="agent-details__band">
              <h2>Agent</h2>
              <dl>
                <div className="agent-details__fact">
                  <dt>Creation origin</dt>
                  <dd>{creationLabel(provenance)}</dd>
                </div>
                <div className="agent-details__fact">
                  <dt>Name</dt>
                  <dd>{detail.title?.trim() || 'Untitled agent'}</dd>
                </div>
                <div className="agent-details__fact">
                  <dt>Model</dt>
                  <dd>{detail.model ?? 'Provider default'}</dd>
                </div>
                <div className="agent-details__fact">
                  <dt>Created</dt>
                  <dd>
                    <time dateTime={detail.created_at}>
                      {new Date(detail.created_at).toLocaleString()}
                    </time>
                  </dd>
                </div>
              </dl>
            </section>
            <section className="agent-details__band">
              <h2>Channel activity</h2>
              <ChannelUsage client={client} id={agent} />
            </section>
            {ready.access.capabilities.includes('view_artifacts') ? (
              <section className="agent-details__band">
                <h2>Published files</h2>
                <ArtifactButton client={client} id={agent} publicationScope={publicationScope} />
              </section>
            ) : null}
          </>
        )}
        <section className="agent-details__band">
          <Button
            size="sm"
            onClick={() => {
              current.reload();
              onAccessChanged?.();
            }}
          >
            Refresh access
          </Button>
        </section>
      </div>
    </aside>
  );
}
