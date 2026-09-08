import { useEffect, useRef, useState } from 'react';
import type { AgentDetail, WorkspaceDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { Button } from '../ui/Button';
import { InlineConfirm } from '../ui/InlineConfirm';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { IconCloseOutline16, IconStopFill16, IconTrashOutline16 } from '../ui/icons';
import type { AgentCommand } from './controller';
import './Details.css';

interface AgentDetailsProps {
  readonly detail?: AgentDetail;
  readonly workspace?: WorkspaceDetail;
  readonly pending?: AgentCommand;
  readonly historyLoading?: boolean;
  readonly actionError?: string;
  readonly open?: boolean;
  readonly onClose?: () => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onStop: () => Promise<void> | void;
  readonly onDelete: () => Promise<void> | void;
}

function label(value: string): string {
  return value.replaceAll('_', ' ').replace(/^./, (letter) => letter.toUpperCase());
}

function ResourceLink({
  route,
  children,
  onNavigate,
}: {
  readonly route: AppRoute;
  readonly children: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
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
      {children}
    </a>
  );
}

function Band({ title, children }: { readonly title: string; readonly children: React.ReactNode }) {
  return (
    <section className="agent-details__band">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function Fact({ term, children }: { readonly term: string; readonly children: React.ReactNode }) {
  return (
    <div className="agent-details__fact">
      <dt>{term}</dt>
      <dd>{children}</dd>
    </div>
  );
}

export function AgentDetails({
  detail,
  workspace,
  pending,
  historyLoading = false,
  actionError,
  open = false,
  onClose,
  onNavigate,
  onStop,
  onDelete,
}: AgentDetailsProps) {
  const [confirmStop, setConfirmStop] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
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
  const live = ['starting', 'ready', 'working', 'waiting_approval', 'paused'].includes(
    detail.status,
  );
  const deletable = detail.status === 'stopped' || detail.status === 'failed';

  return (
    <aside className="agent-details" aria-label="Agent details">
      <header className="agent-details__header">
        <h1 ref={heading} tabIndex={-1}>
          Details
        </h1>
        <span>{label(detail.status)}</span>
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
        <Band title="Sandbox">
          <dl>
            <Fact term="Name">
              <ResourceLink
                route={{ kind: 'sandbox', sandboxId: detail.sandbox.id }}
                onNavigate={onNavigate}
              >
                {detail.sandbox.name}
              </ResourceLink>
            </Fact>
            <Fact term="Status">{label(detail.sandbox.status)}</Fact>
            <Fact term="Profile">{`${detail.sandbox.profile.id} @ ${String(detail.sandbox.profile.revision)}`}</Fact>
            <Fact term="Reach">{detail.reach.length === 0 ? 'None' : detail.reach.join(', ')}</Fact>
          </dl>
        </Band>
        <Band title="Workspace">
          <dl>
            <Fact term="Name">{detail.workspace.name}</Fact>
            <Fact term="Path">
              <code>{detail.workspace.path}</code>
            </Fact>
            <Fact term="Status">{label(detail.workspace.status)}</Fact>
            {workspace === undefined ? null : (
              <Fact term="Source">
                {workspace.source.type === 'empty' ? (
                  'Empty directory'
                ) : (
                  <code>{workspace.source.url}</code>
                )}
              </Fact>
            )}
            {detail.workspace.resolvedCommit === undefined ? null : (
              <Fact term="Commit">
                <code>{detail.workspace.resolvedCommit}</code>
              </Fact>
            )}
          </dl>
        </Band>
        <Band title="Worktree">
          <dl>
            <Fact term="Name">{detail.worktree.name}</Fact>
            <Fact term="Branch">
              <code>{detail.worktree.branch}</code>
            </Fact>
            <Fact term="Path">
              <code>{detail.worktree.path}</code>
            </Fact>
            <Fact term="Base">
              <code>{detail.worktree.baseCommit}</code>
            </Fact>
          </dl>
        </Band>
        <Band title="Agent profile">
          <dl>
            <Fact term="Profile">
              <ResourceLink
                route={{ kind: 'profile', profileKind: 'agent', profileId: detail.profile.id }}
                onNavigate={onNavigate}
              >
                {`${detail.profile.id} @ ${String(detail.profile.revision)}`}
              </ResourceLink>
            </Fact>
            <Fact term="Model">{detail.profile.model ?? 'Provider default'}</Fact>
            <Fact term="Thinking">{label(detail.profile.thinkingEffort)}</Fact>
            <Fact term="Permission">{label(detail.profile.permissionLevel)}</Fact>
            <Fact term="Skills">
              {detail.profile.skills.length === 0
                ? 'None'
                : detail.profile.skills
                    .map(
                      (skill) =>
                        `${skill.source}:${skill.id}${skill.version === undefined ? '' : `@${skill.version}`}`,
                    )
                    .join(', ')}
            </Fact>
          </dl>
        </Band>
        <Band title="First-message delivery">
          {detail.firstMessage === undefined ? (
            <p>Not requested</p>
          ) : (
            <dl>
              <Fact term="State">{label(detail.firstMessage.state)}</Fact>
              <Fact term="Queued">
                <time dateTime={detail.firstMessage.queuedAt}>{detail.firstMessage.queuedAt}</time>
              </Fact>
              {detail.firstMessage.completedAt === undefined ? null : (
                <Fact term="Completed">
                  <time dateTime={detail.firstMessage.completedAt}>
                    {detail.firstMessage.completedAt}
                  </time>
                </Fact>
              )}
            </dl>
          )}
        </Band>
        <Band title="Agent controls">
          {historyLoading ? (
            <p className="agent-details__control-reason" role="status">
              Loading conversation activity.
            </p>
          ) : null}
          {actionError === undefined ? null : (
            <p className="agent-details__failure" role="alert">
              {actionError}
            </p>
          )}
          {confirmStop ? (
            <InlineConfirm
              question="Stop this Agent?"
              confirmLabel="Stop"
              busy={historyLoading || pending === 'stop'}
              onCancel={() => setConfirmStop(false)}
              onConfirm={() => {
                setConfirmStop(false);
                void onStop();
              }}
            />
          ) : (
            <div className="agent-details__actions">
              <Button
                icon={<IconStopFill16 />}
                disabled={historyLoading || !live || pending !== undefined}
                onClick={() => setConfirmStop(true)}
              >
                Stop
              </Button>
              <Button
                variant="danger"
                icon={<IconTrashOutline16 />}
                disabled={historyLoading || !deletable || pending !== undefined}
                onClick={() => setConfirmDelete(true)}
              >
                Delete
              </Button>
            </div>
          )}
          {deletable ? null : (
            <p className="agent-details__control-reason">Stop the Agent before deleting it.</p>
          )}
        </Band>
      </div>
      <RiskConfirmation
        open={confirmDelete}
        title="Delete Agent"
        description={`Delete ${detail.title ?? detail.id} and its Worktree. This cannot be undone.`}
        acknowledgeLabel="I understand this Agent and Worktree will be deleted."
        confirmLabel="Delete Agent"
        busy={historyLoading || pending === 'delete'}
        failure={actionError}
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => void onDelete()}
      />
    </aside>
  );
}
