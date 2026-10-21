import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxProfileSettings } from '../api/types';
import { useOwnerViews, type SandboxContents } from '../resources/retainedOwnerViews';

/**
 * The owner's view of what a Sandbox holds: its pinned settings, its
 * Workspaces and their Agents.
 *
 * Several parts of the Sandbox page read the same children, so they are
 * loaded once here. Values start from the retained owner view when it belongs
 * to the same viewer and client, and a partial failure is reported rather
 * than shown as an empty Sandbox. Callers key the component that owns this
 * hook by Sandbox, owner and pinned profile, so none of these values can
 * outlive the identity they were read for.
 */
export function useSandboxContents({
  client,
  sandbox,
  readEnabled,
}: {
  readonly client: CmaWebClient;
  readonly sandbox: SandboxDetail;
  readonly readEnabled: boolean;
}) {
  const ownerViews = useOwnerViews();
  const views =
    ownerViews?.client === client &&
    sandbox.owner === ownerViews.viewerId &&
    !['retiring', 'retired'].includes(sandbox.status)
      ? ownerViews
      : undefined;
  const profileKey = JSON.stringify(sandbox.profile);
  const retainedSettings = views?.settings.get(sandbox.id);
  const [profileSettings, setSettings] = useState<SandboxProfileSettings | undefined>(() =>
    retainedSettings?.profileKey === profileKey ? retainedSettings.value : undefined,
  );
  const [settingsLoaded, setSettingsLoaded] = useState(profileSettings !== undefined);
  const [contents, setContents] = useState<SandboxContents | undefined>(() =>
    views?.contents.get(sandbox.id),
  );
  const [retry, setRetry] = useState(0);
  const { agentProvider, modelAccess } = sandbox;
  const hasPinnedSettings = agentProvider !== undefined && modelAccess !== undefined;
  const settings = hasPinnedSettings
    ? { agentProvider, modelAccess, nyxidServices: sandbox.nyxidServices ?? [] }
    : profileSettings;
  const { id: profileId, revision: profileRevision, digest: profileDigest } = sandbox.profile;
  useEffect(() => {
    if (hasPinnedSettings || !readEnabled) return;
    const abort = new AbortController();
    void client
      .readSandboxProfileSettings(
        { id: profileId, revision: profileRevision, digest: profileDigest },
        { signal: abort.signal },
      )
      .then((value) => {
        if (!abort.signal.aborted) {
          setSettings(value);
          if (JSON.stringify(views?.sandboxes.get(sandbox.id)?.sandbox.profile) === profileKey)
            views?.settings.put(sandbox.id, { profileKey, value });
        }
      })
      .catch(() => {
        if (!abort.signal.aborted) {
          setSettings(undefined);
          views?.settings.delete(sandbox.id);
        }
      })
      .finally(() => {
        if (!abort.signal.aborted) setSettingsLoaded(true);
      });
    return () => abort.abort();
  }, [
    client,
    profileId,
    profileRevision,
    profileDigest,
    hasPinnedSettings,
    retry,
    readEnabled,
    views,
    sandbox.id,
    profileKey,
  ]);
  useEffect(() => {
    if (!readEnabled) return;
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
      if (results.some((result) => result.status === 'rejected'))
        views?.contents.delete(sandbox.id);
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
      if (!abort.signal.aborted) {
        const value = {
          workspaces,
          agents: agentResults.flatMap((result) =>
            result.status === 'fulfilled' ? [result.value] : [],
          ),
          failed: [...results, ...agentResults].some((result) => result.status === 'rejected'),
        };
        setContents(value);
        if (value.failed) views?.contents.delete(sandbox.id);
        else if (views?.sandboxes.get(sandbox.id)?.sandbox === sandbox)
          views.contents.put(sandbox.id, value);
      }
    })();
    return () => abort.abort();
  }, [client, sandbox, retry, views, readEnabled]);
  return {
    settings,
    settingsLoaded,
    contents,
    retry: () => setRetry((value) => value + 1),
  };
}
