import type { ReactNode } from 'react';
import type { SandboxDetail, SandboxProfileSettings } from '../api/types';
import { Button } from '../ui/Button';
import { codexProviderName } from '../profiles/modelProvider';
import { Resources } from '../agent/Resources';
import type { SandboxContents } from '../resources/retainedOwnerViews';

/**
 * The Overview tab: the runtime, live machine resources and the facts of the
 * environment Agents run in — `docs/Web_Style.md` §17.
 */
export function SandboxOverview({
  sandbox,
  contents,
  settings,
  settingsLoaded,
  onRetry,
  runtime,
  busy,
  stale = false,
}: {
  readonly sandbox: SandboxDetail;
  readonly contents?: SandboxContents;
  readonly settings?: SandboxProfileSettings;
  readonly settingsLoaded: boolean;
  readonly onRetry: () => void;
  /** The owner's Runtime card, when the public update projection is available. */
  readonly runtime?: ReactNode;
  readonly busy: boolean;
  readonly stale?: boolean;
}) {
  const resourceAgent = contents?.agents.find((agent) => agent.status !== 'deleted');
  const workspaces = sandbox.workspaces.filter((workspace) => workspace.status !== 'removed');
  return (
    <>
      {runtime}
      <section className="sandbox-page__band">
        <h2>Machine resources</h2>
        {resourceAgent === undefined ? (
          <p>Resources are available when this Sandbox has an Agent.</p>
        ) : (
          <Resources
            agentId={resourceAgent.id}
            sandboxId={sandbox.id}
            sandboxProfile={sandbox.profile}
            availability={
              sandbox.status === 'active' &&
              sandbox.runtimeVersion &&
              !busy &&
              !stale &&
              sandbox.lifecycle !== undefined &&
              !sandbox.lifecycle.checkpoint &&
              !sandbox.lifecycle?.operation
                ? { state: 'live', runtimeVersion: sandbox.runtimeVersion }
                : sandbox.status === 'paused'
                  ? {
                      state: 'paused',
                      notice: 'Sandbox paused. Live resources resume when it is active.',
                    }
                  : {
                      state: 'unavailable',
                      notice: 'Live resources will be available when the Sandbox is active.',
                    }
            }
          />
        )}
      </section>
      <section className="sandbox-page__band">
        <h2>Environment</h2>
        <dl>
          <div>
            <dt>Owner</dt>
            <dd>{sandbox.ownerName?.trim() || 'Name unavailable'}</dd>
          </div>
          <div>
            <dt>Model access</dt>
            <dd>
              {settings === undefined
                ? settingsLoaded
                  ? 'Unavailable'
                  : 'Loading'
                : codexProviderName(settings.modelAccess)}
            </dd>
          </div>
          <div>
            <dt>Workspaces</dt>
            <dd>
              {workspaces.length} of {sandbox.limits.maxWorkspaces}
            </dd>
          </div>
          <div>
            <dt>Agents per workspace</dt>
            <dd>Up to {sandbox.limits.maxAgentsPerWorkspace}</dd>
          </div>
        </dl>
        {settingsLoaded && settings === undefined ? (
          <p>
            Agent access settings could not be loaded.{' '}
            <Button size="sm" onClick={onRetry}>
              Try again
            </Button>
          </p>
        ) : null}
      </section>
    </>
  );
}
