import type { ReactNode } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { LifecycleProgress } from './LifecycleProgress';
import { SandboxAccess } from './SandboxAccess';
import { SandboxActivity } from './SandboxActivity';
import { SandboxOverview } from './SandboxOverview';
import { SandboxInterruptionHistory } from './SandboxRecoveryNotice';
import { SandboxTabs } from './SandboxTabs';
import { SandboxWorkspaces } from './SandboxWorkspaces';
import type { SandboxTab } from './sandboxTabRoute';
import { useSandboxContents } from './useSandboxContents';

/** An active Sandbox with nothing recorded in flight has no progress to follow. */
export function sandboxQuiet(sandbox: SandboxDetail): boolean {
  const lifecycle = sandbox.lifecycle;
  return (
    sandbox.status === 'active' &&
    lifecycle?.operation === undefined &&
    lifecycle?.checkpoint === undefined &&
    lifecycle?.backgroundSave === undefined
  );
}

/**
 * The tabbed body of the owner Sandbox page — `docs/Web_Style.md` §17.
 *
 * It owns the reads of the Sandbox's settings and children, which every tab
 * shares, so its owner keys it by Sandbox, owner and pinned profile.
 */
export function SandboxSections({
  client,
  sandbox,
  owner,
  observedAt,
  permissions,
  busy,
  stale,
  readEnabled,
  createReason,
  runtime,
  selected,
  onSelect,
  onAction,
  onNavigate,
}: {
  readonly client: CmaWebClient;
  readonly sandbox: SandboxDetail;
  /** The signed-in viewer, when they own this Sandbox. */
  readonly owner?: string;
  readonly observedAt: string;
  readonly permissions: TreePermissions;
  readonly busy: boolean;
  readonly stale: boolean;
  readonly readEnabled: boolean;
  readonly createReason?: string;
  readonly runtime?: ReactNode;
  readonly selected: SandboxTab;
  readonly onSelect: (tab: SandboxTab) => void;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const { settings, settingsLoaded, contents, retry } = useSandboxContents({
    client,
    sandbox,
    readEnabled,
  });
  return (
    <SandboxTabs
      label="Sandbox sections"
      selected={selected}
      onSelect={onSelect}
      tabs={[
        {
          id: 'overview',
          label: 'Overview',
          content: (
            <SandboxOverview
              sandbox={sandbox}
              contents={contents}
              settings={settings}
              settingsLoaded={settingsLoaded}
              onRetry={retry}
              runtime={runtime}
              busy={busy}
              stale={stale}
            />
          ),
        },
        {
          id: 'workspaces',
          label: 'Workspaces',
          content: (
            <SandboxWorkspaces
              sandbox={sandbox}
              contents={contents}
              permissions={permissions}
              createReason={createReason}
              onRetry={retry}
              onAction={onAction}
              onNavigate={onNavigate}
            />
          ),
        },
        {
          id: 'access',
          label: 'Access',
          content: (
            <SandboxAccess
              sandbox={sandbox}
              contents={contents}
              settings={settings}
              owner={owner}
              active={selected === 'access'}
              onRetry={retry}
            />
          ),
        },
        {
          id: 'activity',
          label: 'Activity',
          lazy: true,
          content: (
            <section className="sandbox-page__band" aria-label="Activity">
              <h2>Activity</h2>
              {sandboxQuiet(sandbox) ? (
                <>
                  <p>No Sandbox operation is in progress.</p>
                  <SandboxActivity client={client} sandboxId={sandbox.id} settled />
                </>
              ) : (
                <LifecycleProgress
                  client={client}
                  sandbox={sandbox}
                  observedAt={observedAt}
                  stale={stale}
                />
              )}
              <SandboxInterruptionHistory sandbox={sandbox} />
            </section>
          ),
        },
      ]}
    />
  );
}
