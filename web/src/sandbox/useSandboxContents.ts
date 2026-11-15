import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { detailReadSource } from '../api/detailReads';
import { PROLONGED_POLL_MS, unchangedPollMaximum } from './pollDelay';
import type { SandboxDetail, SandboxProfileSettings } from '../api/types';
import { useOwnerViews, type SandboxContents } from '../resources/retainedOwnerViews';

interface ContentsObservation {
  readonly key: string;
  readonly fingerprint: string;
  readonly unchangedSince: number;
  readonly refreshAt: number;
}

// Metadata follows the retained value's lifetime and cannot cross owner views.
const observations = new WeakMap<SandboxContents, ContentsObservation>();

function contentsKey(sandbox: SandboxDetail) {
  return JSON.stringify([
    sandbox.id,
    sandbox.revision,
    sandbox.workspaces.map(({ id, status, agentCount, worktreeCount }) =>
      [id, status, agentCount, worktreeCount].join(':'),
    ),
  ]);
}

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
    ownerViews?.client === detailReadSource(client) &&
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
    // Settings are pinned to an immutable profile revision, so a retained
    // successful read stays valid until the owner explicitly retries.
    if (retry === 0 && views?.settings.get(sandbox.id)?.profileKey === profileKey) return;
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
  const sandboxId = sandbox.id;
  const key = contentsKey(sandbox);
  const workspaceIds = JSON.stringify(
    sandbox.workspaces.filter((workspace) => workspace.status !== 'removed').map((row) => row.id),
  );
  const handledRetry = useRef(0);
  useEffect(() => {
    if (!readEnabled) return;
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    const cached = views?.contents.get(sandboxId);
    const saved = cached === undefined ? undefined : observations.get(cached);
    let observation = saved?.key === key && handledRetry.current === retry ? saved : undefined;
    handledRetry.current = retry;
    // projection.rs exposes Workspace status/counts in the Sandbox summary,
    // but Agent status, titles and task progress only in child details. A stable
    // parent revision therefore still needs this bounded child observation.
    const refresh = async () => {
      if (abort.signal.aborted || document.hidden || inFlight) return;
      clearTimeout(timer);
      inFlight = true;
      const ids: string[] = JSON.parse(workspaceIds);
      const results = await Promise.allSettled(
        ids.map(async (id) => {
          const workspace = await client.readWorkspace(id, { signal: abort.signal });
          if (workspace.id !== id || workspace.sandboxId !== sandboxId)
            throw new Error('Workspace identity mismatch');
          return workspace;
        }),
      );
      if (abort.signal.aborted) return;
      const workspaces = results.flatMap((result) =>
        result.status === 'fulfilled' ? [result.value] : [],
      );
      if (results.some((result) => result.status === 'rejected')) views?.contents.delete(sandboxId);
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
        failed: previous?.failed === true || results.some((result) => result.status === 'rejected'),
      }));
      const agentResults = await Promise.allSettled(
        workspaces.flatMap((workspace) =>
          workspace.agents
            .filter((agent) => agent.status !== 'deleted')
            .map(async (row) => {
              const agent = await client.readAgent(row.id, { signal: abort.signal });
              if (
                agent.id !== row.id ||
                agent.sandbox.id !== sandboxId ||
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
        if (value.failed) views?.contents.delete(sandboxId);
        const now = Date.now();
        const fingerprint = JSON.stringify(value);
        const unchangedSince =
          observation?.fingerprint === fingerprint ? observation.unchangedSince : now;
        const delay = value.failed
          ? 5000
          : Math.max(PROLONGED_POLL_MS, unchangedPollMaximum(now - unchangedSince));
        observation = value.failed
          ? undefined
          : {
              key,
              fingerprint,
              unchangedSince,
              refreshAt: now + delay,
            };
        const parent = views?.sandboxes.get(sandboxId)?.sandbox;
        if (observation !== undefined && parent !== undefined && contentsKey(parent) === key) {
          observations.set(value, observation);
          views?.contents.put(sandboxId, value);
        }
        timer = setTimeout(() => void refresh(), delay);
      }
      inFlight = false;
    };
    const visible = () => {
      clearTimeout(timer);
      if (document.hidden) return;
      observation = undefined;
      void refresh();
    };
    document.addEventListener('visibilitychange', visible);
    const remaining = (observation?.refreshAt ?? 0) - Date.now();
    if (remaining > 0) timer = setTimeout(() => void refresh(), remaining);
    else void refresh();
    return () => {
      abort.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [client, sandboxId, key, workspaceIds, retry, views, readEnabled]);
  return {
    settings,
    settingsLoaded,
    contents,
    retry: () => setRetry((value) => value + 1),
  };
}
