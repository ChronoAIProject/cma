import { useState, type ReactNode } from 'react';
import type { SandboxDetail, SandboxProfileSettings } from '../api/types';
import { Button } from '../ui/Button';
import { codexProviderName } from '../profiles/modelProvider';
import { Resources } from '../agent/Resources';
import type { SandboxContents } from '../resources/retainedOwnerViews';

/**
 * The Overview tab: the runtime, live machine resources and the facts of the
 * environment Agents run in — the shared style contracts
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
  onAlwaysOnChange,
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
  readonly onAlwaysOnChange?: (alwaysOn: boolean) => Promise<void>;
}) {
  const [alwaysOnBusy, setAlwaysOnBusy] = useState(false);
  const [alwaysOnError, setAlwaysOnError] = useState<string>();
  const resourceAgent = contents?.agents.find((agent) => agent.status !== 'deleted');
  const workspaces = sandbox.workspaces.filter((workspace) => workspace.status !== 'removed');
  return (
    <>
      {runtime}
      <section className="sandbox-page__band">
        {onAlwaysOnChange === undefined ? null : (
          <div>
            <h2>Sandbox controls</h2>
            <h3>Always on</h3>
            <p className="sandbox-page__hint">
              {sandbox.alwaysOn === true
                ? 'An administrator approved Always on, so automatic pause skips this Sandbox.'
                : 'Always on is off. Only an administrator can approve an exception.'}
            </p>
            {sandbox.alwaysOn === true ? (
              <Button
                size="sm"
                disabled={alwaysOnBusy || busy || stale}
                onClick={() => {
                  setAlwaysOnBusy(true);
                  setAlwaysOnError(undefined);
                  void onAlwaysOnChange(false)
                    .catch((error: unknown) => {
                      setAlwaysOnError(
                        error instanceof Error ? error.message : 'Could not turn off Always on.',
                      );
                    })
                    .finally(() => setAlwaysOnBusy(false));
                }}
              >
                Turn off Always on
              </Button>
            ) : null}
            {alwaysOnError === undefined ? null : <p role="alert">{alwaysOnError}</p>}
          </div>
        )}
      </section>
      <section className="sandbox-page__band">
        <h2>Machine resources</h2>
        <p className="sandbox-page__hint">
          How much memory, processing power and network the Agent’s computer is using.
        </p>
        {sandbox.status === 'retired' ? (
          <p>This environment is no longer running. Live resource usage is no longer available.</p>
        ) : sandbox.status === 'retiring' ? (
          <p>Live resource usage is unavailable while this environment is retiring.</p>
        ) : resourceAgent === undefined ? (
          <p>Resources are available when this Sandbox has an Agent.</p>
        ) : (
          <Resources
            agentId={resourceAgent.id}
            sandboxId={sandbox.id}
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
