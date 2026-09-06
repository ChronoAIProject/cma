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
    const refresh = async () => {
      onReady(false);
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
        onReady(detail.status === 'active' && (!workspace || selected?.status === 'ready'));
        if (
          ['provisioning', 'installing', 'reviving'].includes(detail.status) ||
          detail.workspaces.some((item) => item.status === 'preparing')
        )
          timer = setTimeout(() => void refresh(), 1500);
      } catch {
        if (!request.signal.aborted) {
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
      : sandboxes;
  const current = sandbox?.id === value.sandboxId ? sandbox : undefined;
  return (
    <div className="create-form">
      <div className="create-form__field">
        <label id="create-sandbox-label">Sandbox</label>
        <Picker
          label="Sandbox"
          value={value.sandboxId ?? ''}
          disabled={disabled}
          options={[
            { value: '', label: 'Choose a sandbox' },
            ...all.map((item) => ({
              value: item.id,
              label: item.name,
              unavailable: ['failed', 'retiring', 'retired', 'paused'].includes(item.status)
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
      {current === undefined || current.status === 'active' ? null : (
        <p role="status">Sandbox: {statusLabel(current.status)}</p>
      )}
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
          <label>Workspace</label>
          <Picker
            label="Workspace"
            value={value.workspaceId ?? ''}
            disabled={disabled || current === undefined}
            options={[
              { value: '', label: 'Choose a workspace' },
              ...(current?.workspaces ?? []).map((item) => ({
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
            disabled={disabled || current?.status !== 'active'}
            onClick={onNewWorkspace}
          >
            New workspace
          </Button>
        )}
      </div>
      {error === undefined ? null : (
        <div role="alert">
          <p>{error}</p>
          <Button disabled={disabled} onClick={() => setRevision((value) => value + 1)}>
            Try again
          </Button>
        </div>
      )}
    </div>
  );
}
