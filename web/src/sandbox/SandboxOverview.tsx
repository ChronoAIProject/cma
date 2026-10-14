import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type {
  AgentDetail,
  SandboxDetail,
  SandboxProfileSettings,
  WorkspaceDetail,
} from '../api/types';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { Button } from '../ui/Button';
import { IconPlusOutline16 } from '../ui/icons';
import { ReferenceLink } from '../profiles/ReferenceLink';
import { sandboxChildrenAvailable, sandboxChildrenReadable, sandboxChildStatus } from './lifecycle';
import { statusLabel } from '../vocabulary/status';
import { codexProviderName, isCodexConnection } from '../profiles/modelProvider';

export function SandboxOverview({
  client,
  sandbox,
  permissions,
  busy,
  onAction,
  onNavigate,
}: {
  readonly client: CmaWebClient;
  readonly sandbox: SandboxDetail;
  readonly permissions: TreePermissions;
  readonly busy: boolean;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [profileSettings, setSettings] = useState<SandboxProfileSettings>();
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [contents, setContents] = useState<{
    workspaces: readonly WorkspaceDetail[];
    agents: readonly AgentDetail[];
    failed: boolean;
  }>();
  const [retry, setRetry] = useState(0);
  const { agentProvider, modelAccess } = sandbox;
  const hasPinnedSettings = agentProvider !== undefined && modelAccess !== undefined;
  const settings = hasPinnedSettings
    ? { agentProvider, modelAccess, nyxidServices: sandbox.nyxidServices ?? [] }
    : profileSettings;
  const { id: profileId, revision: profileRevision, digest: profileDigest } = sandbox.profile;
  useEffect(() => {
    if (hasPinnedSettings) return;
    const abort = new AbortController();
    void client
      .readSandboxProfileSettings(
        { id: profileId, revision: profileRevision, digest: profileDigest },
        { signal: abort.signal },
      )
      .then((value) => {
        if (!abort.signal.aborted) setSettings(value);
      })
      .catch(() => {
        if (!abort.signal.aborted) setSettings(undefined);
      })
      .finally(() => {
        if (!abort.signal.aborted) setSettingsLoaded(true);
      });
    return () => abort.abort();
  }, [client, profileId, profileRevision, profileDigest, hasPinnedSettings, retry]);
  useEffect(() => {
    const abort = new AbortController();
    void (async () => {
      const results = await Promise.allSettled(
        sandbox.workspaces
          .filter((workspace) => workspace.status !== 'removed')
          .map(async (row) => {
            const workspace = await client.readWorkspace(row.id, { signal: abort.signal });
            if (workspace.id !== row.id || workspace.sandboxId !== sandbox.id)
              throw new Error('Workspace identity mismatch');
            return workspace;
          }),
      );
      if (abort.signal.aborted) return;
      const workspaces = results.flatMap((result) =>
        result.status === 'fulfilled' ? [result.value] : [],
      );
      setContents((previous) => ({
        workspaces,
        agents:
          previous?.agents.filter((agent) =>
            workspaces.some((workspace) =>
              workspace.agents.some(
                (row) =>
                  row.id === agent.id &&
                  row.profile.id === agent.profile.id &&
                  row.profile.revision === agent.profile.revision,
              ),
            ),
          ) ?? [],
        failed: results.some((result) => result.status === 'rejected'),
      }));
      const agentResults = await Promise.allSettled(
        workspaces.flatMap((workspace) =>
          workspace.agents
            .filter((agent) => agent.status !== 'deleted')
            .map(async (row) => {
              const agent = await client.readAgent(row.id, { signal: abort.signal });
              if (
                agent.id !== row.id ||
                agent.sandbox.id !== sandbox.id ||
                agent.workspace.id !== workspace.id ||
                agent.worktree.id !== row.worktreeId
              )
                throw new Error('Agent identity mismatch');
              return agent;
            }),
        ),
      );
      if (!abort.signal.aborted)
        setContents({
          workspaces,
          agents: agentResults.flatMap((result) =>
            result.status === 'fulfilled' ? [result.value] : [],
          ),
          failed: [...results, ...agentResults].some((result) => result.status === 'rejected'),
        });
    })();
    return () => abort.abort();
  }, [client, sandbox, retry]);
  const reach = (sandbox.nyxidServices ?? contents?.agents[0]?.reach)?.filter(
    (id) => !isCodexConnection(id),
  );
  const available = sandboxChildrenAvailable(sandbox);
  return (
    <>
      <section className="sandbox-page__band">
        <h2>Agent access</h2>
        <dl>
          <div>
            <dt>Agent provider</dt>
            <dd>
              {settings === undefined ? (settingsLoaded ? 'Unavailable' : 'Loading') : 'Codex'}
            </dd>
          </div>
          <div>
            <dt>Codex provider</dt>
            <dd>
              {settings === undefined
                ? settingsLoaded
                  ? 'Unavailable'
                  : 'Loading'
                : codexProviderName(settings.modelAccess)}
            </dd>
          </div>
          <div>
            <dt>NyxID services</dt>
            <dd>
              {reach === undefined ? (
                'Access not yet confirmed'
              ) : reach.length === 0 ? (
                'None'
              ) : (
                <ul className="sandbox-page__references">
                  {reach.map((id) => (
                    <li key={id}>
                      <ReferenceLink kind="service" id={id} />
                    </li>
                  ))}
                </ul>
              )}
              {reach === undefined &&
              settings !== undefined &&
              settings.nyxidServices.length > 0 ? (
                <>
                  <p>Requested services</p>
                  <ul className="sandbox-page__references">
                    {settings.nyxidServices
                      .filter((id) => !isCodexConnection(id))
                      .map((id) => (
                        <li key={id}>
                          <ReferenceLink kind="service" id={id} />
                        </li>
                      ))}
                  </ul>
                </>
              ) : null}
            </dd>
          </div>
        </dl>
        {settingsLoaded && settings === undefined ? (
          <p>
            Agent access settings could not be loaded.{' '}
            <Button size="sm" onClick={() => setRetry((value) => value + 1)}>
              Try again
            </Button>
          </p>
        ) : null}
      </section>
      {sandboxChildrenReadable(sandbox) ? (
        <section className="sandbox-page__band">
          <h2>Workspaces</h2>
          {contents === undefined ? (
            <p>Loading workspaces</p>
          ) : (
            <>
              {contents.failed ? (
                <p>
                  Some workspace or agent details are unavailable.{' '}
                  <Button size="sm" onClick={() => setRetry((value) => value + 1)}>
                    Try again
                  </Button>
                </p>
              ) : null}
              {contents.workspaces.length === 0 && !contents.failed ? <p>No workspaces</p> : null}
              {contents.workspaces.map((workspace) => (
                <section className="sandbox-page__workspace" key={workspace.id}>
                  <header className="sandbox-page__header">
                    <div>
                      <h3>{workspace.name}</h3>
                      <p>{statusLabel(sandboxChildStatus(sandbox, workspace.status))}</p>
                    </div>
                    {permissions.createAgent ? (
                      <Button
                        icon={<IconPlusOutline16 />}
                        disabled={busy || !available || workspace.status !== 'ready'}
                        onClick={() => onAction({ kind: 'new_agent', sandboxId: sandbox.id })}
                      >
                        New agent
                      </Button>
                    ) : null}
                  </header>
                  {workspace.status === 'removing' ? null : (
                    <ul className="sandbox-page__agents">
                      {workspace.agents
                        .filter((agent) => agent.status !== 'deleted')
                        .map((agent) => {
                          const detail = contents.agents.find((item) => item.id === agent.id);
                          const route = { kind: 'agent', agentId: agent.id } as const;
                          return (
                            <li key={agent.id}>
                              <div className="sandbox-page__header">
                                <a
                                  href={routePath(route)}
                                  onClick={(event) => {
                                    if (
                                      event.button !== 0 ||
                                      event.metaKey ||
                                      event.ctrlKey ||
                                      event.shiftKey ||
                                      event.altKey
                                    )
                                      return;
                                    event.preventDefault();
                                    onNavigate(route);
                                  }}
                                >
                                  {agent.title?.trim() || 'Untitled agent'}
                                </a>
                                <span>
                                  {statusLabel(sandboxChildStatus(sandbox, agent.status))}
                                </span>
                              </div>
                              {detail === undefined ? (
                                <p>Agent settings unavailable</p>
                              ) : (
                                <dl>
                                  <div>
                                    <dt>Model</dt>
                                    <dd>{detail.profile.model ?? 'Provider default'}</dd>
                                  </div>
                                  <div>
                                    <dt>Permissions</dt>
                                    <dd>
                                      {detail.profile.permissionLevel === 'guarded'
                                        ? 'Guarded'
                                        : detail.profile.permissionLevel === 'autonomous'
                                          ? 'Autonomous'
                                          : 'Not reported'}
                                    </dd>
                                  </div>
                                  <div>
                                    <dt>Skills</dt>
                                    <dd>
                                      {detail.profile.skills.length === 0 ? (
                                        'None'
                                      ) : (
                                        <ul className="sandbox-page__references">
                                          {detail.profile.skills.map((skill) => (
                                            <li key={`${skill.source}:${skill.id}`}>
                                              <ReferenceLink
                                                kind="skill"
                                                id={skill.id}
                                                version={skill.version}
                                              />
                                            </li>
                                          ))}
                                        </ul>
                                      )}
                                    </dd>
                                  </div>
                                </dl>
                              )}
                            </li>
                          );
                        })}
                    </ul>
                  )}
                  {workspace.status !== 'removing' &&
                  workspace.agents.every((agent) => agent.status === 'deleted') ? (
                    <p>No agents</p>
                  ) : null}
                </section>
              ))}
            </>
          )}
        </section>
      ) : null}
    </>
  );
}
