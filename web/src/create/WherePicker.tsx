import { Presence } from '../ui/Presence';
import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxEvent, SandboxSummary, WorkspaceDetail } from '../api/types';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import { IconPlusOutline16 } from '../ui/icons';
import { statusLabel } from '../vocabulary/status';
import { ProvisioningProgress } from '../sandbox/ProvisioningProgress';
import { sandboxProgress, workspaceProgress } from '../sandbox/progress';
import { readSandboxHistory } from '../sandbox/eventHistory';
import { LifecycleProgress } from '../sandbox/LifecycleProgress';
import { sandboxLifecycleLabel, sandboxRefreshDelay } from '../sandbox/lifecycle';

export interface LocationChoice {
  readonly sandboxId?: string;
  readonly workspaceId?: string;
}

export function WherePicker({
  client,
  value,
  onChange,
  onReady,
  onNewSandbox,
  onNewWorkspace,
  workspace = true,
  disabled = false,
}: {
  readonly client: CmaWebClient;
  readonly value: LocationChoice;
  readonly onChange: (value: LocationChoice) => void;
  readonly onReady: (ready: boolean) => void;
  readonly onNewSandbox?: () => void;
  readonly onNewWorkspace?: () => void;
  readonly workspace?: boolean;
  readonly disabled?: boolean;
}) {
  const [sandboxes, setSandboxes] = useState<readonly SandboxSummary[]>([]);
  const [sandbox, setSandbox] = useState<SandboxDetail>();
  const [selectedWorkspace, setSelectedWorkspace] = useState<WorkspaceDetail>();
  const [events, setEvents] = useState<readonly SandboxEvent[]>([]);
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    const request = new AbortController();
    void (async () => {
      const items: SandboxSummary[] = [];
      const seen = new Set<string>();
      let cursor: string | undefined;
      do {
        const page = await client.listSandboxes({ cursor, signal: request.signal });
        items.push(...page.items);
        if (page.next !== undefined && seen.has(page.next))
          throw new Error('Pagination did not advance.');
        cursor = page.next;
        if (cursor !== undefined) seen.add(cursor);
      } while (cursor !== undefined);
      if (!request.signal.aborted) setSandboxes(items);
    })().catch(() => {
      if (!request.signal.aborted) setError('Sandboxes could not be loaded.');
    });
    return () => request.abort();
  }, [client, revision]);
  useEffect(() => {
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let history: readonly SandboxEvent[] = [];
    let initial = true;
    const refresh = async () => {
      if (initial) onReady(false);
      initial = false;
      try {
        if (value.sandboxId === undefined) {
          if (value.workspaceId !== undefined) {
            const item = await client.readWorkspace(value.workspaceId, { signal: request.signal });
            if (!request.signal.aborted)
              onChange({ sandboxId: item.sandboxId, workspaceId: item.id });
          }
          return;
        }
        const detail = await client.readSandbox(value.sandboxId, { signal: request.signal });
        const selected = detail.workspaces.find((item) => item.id === value.workspaceId);
        const workspaceDetail =
          selected === undefined
            ? undefined
            : await client.readWorkspace(selected.id, { signal: request.signal });
        if (detail.status !== 'active' || workspaceDetail?.status === 'preparing')
          history = await readSandboxHistory(client, detail.id, request.signal, history);
        if (request.signal.aborted) return;
        setSandbox(detail);
        setSelectedWorkspace(workspaceDetail);
        setEvents(history);
        setError(undefined);
        onReady(
          acceptsCreation(detail.status) && (!workspace || workspaceDetail?.status === 'ready'),
        );
        timer = setTimeout(
          () => void refresh(),
          sandboxRefreshDelay(
            detail,
            detail.workspaces.some((item) => item.status === 'preparing'),
          ),
        );
      } catch {
        if (!request.signal.aborted) {
          onReady(false);
          setError('This location could not be loaded.');
          timer = setTimeout(() => void refresh(), 5000);
        }
      }
    };
    void refresh();
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [client, value.sandboxId, value.workspaceId, workspace, onChange, onReady, revision]);
  const all =
    sandbox !== undefined && !sandboxes.some((item) => item.id === sandbox.id)
      ? [...sandboxes, sandbox]
      : sandboxes.map((item) => (item.id === sandbox?.id ? sandbox : item));
  const current = sandbox?.id === value.sandboxId ? sandbox : undefined;
  return (
    <div className="create-form">
      <div className="create-form__field">
        <label id="create-sandbox-label">Sandbox environment</label>
        <p className="profile-field__help">
          The cloud environment where your agent runs, with its software and connected services.
        </p>
        <Picker
          label="Sandbox"
          value={value.sandboxId ?? ''}
          disabled={disabled}
          options={[
            { value: '', label: 'Choose a sandbox' },
            ...all
              .filter((item) => !['retiring', 'retired'].includes(item.status))
              .map((item) => ({
                value: item.id,
                label: item.name,
                unavailable: ['failed', 'retiring', 'retired'].includes(item.status)
                  ? statusLabel(item.status)
                  : undefined,
              })),
          ]}
          onChange={(sandboxId) => {
            onReady(false);
            onChange({ sandboxId: sandboxId || undefined });
          }}
        />
      </div>
      <Presence>
        {current === undefined || current.status === 'active' ? null : (
          <p role="status">Sandbox: {sandboxLifecycleLabel(current)}</p>
        )}
      </Presence>
      {current === undefined ? null : <LifecycleProgress client={client} sandbox={current} />}
      {current !== undefined &&
      ['provisioning', 'installing', 'failed'].includes(current.status) ? (
        <ProvisioningProgress
          title="Sandbox preparation"
          steps={sandboxProgress(current, events)}
          failure={current.failure}
        />
      ) : null}
      {workspace ? (
        <div className="create-form__field">
          <label>Project workspace</label>
          <p className="profile-field__help">
            The project or folder your agent will work in. Each agent gets its own working copy.
          </p>
          <Picker
            label="Workspace"
            value={value.workspaceId ?? ''}
            disabled={
              disabled ||
              current === undefined ||
              ['retiring', 'retired', 'failed'].includes(current.status)
            }
            options={[
              { value: '', label: 'Choose a workspace' },
              ...(current?.workspaces ?? [])
                .map((item) => (item.id === selectedWorkspace?.id ? selectedWorkspace : item))
                .filter((item) => !['removing', 'removed'].includes(item.status))
                .map((item) => ({
                  value: item.id,
                  label: item.name,
                  unavailable: item.status === 'ready' ? undefined : statusLabel(item.status),
                })),
            ]}
            onChange={(workspaceId) => {
              onReady(false);
              onChange({ sandboxId: value.sandboxId, workspaceId: workspaceId || undefined });
            }}
          />
        </div>
      ) : null}
      {selectedWorkspace !== undefined &&
      selectedWorkspace.id === value.workspaceId &&
      ['preparing', 'failed'].includes(selectedWorkspace.status) ? (
        <ProvisioningProgress
          title="Workspace preparation"
          steps={workspaceProgress(selectedWorkspace, events)}
          failure={selectedWorkspace.failure}
        />
      ) : null}
      <div className="create-form__actions">
        {onNewSandbox === undefined ? null : (
          <Button icon={<IconPlusOutline16 />} disabled={disabled} onClick={onNewSandbox}>
            New sandbox
          </Button>
        )}
        {onNewWorkspace === undefined ? null : (
          <Button
            icon={<IconPlusOutline16 />}
            disabled={disabled || current === undefined || !acceptsCreation(current.status)}
            onClick={onNewWorkspace}
          >
            New workspace
          </Button>
        )}
      </div>
      <Presence>
        {error === undefined ? null : (
          <div role="alert">
            <p>{error}</p>
            <Button disabled={disabled} onClick={() => setRevision((value) => value + 1)}>
              Try again
            </Button>
          </div>
        )}
      </Presence>
    </div>
  );
}

function acceptsCreation(status: SandboxSummary['status']): boolean {
  return ['active', 'paused', 'reviving'].includes(status);
}
