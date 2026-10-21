import { Presence } from '../ui/Presence';
import { useEffect, useState } from 'react';
import type { Launch, TriggerPreview } from '@chronoai/cma-sdk';
import { Alert } from '../ui/Alert';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { SkeletonRows } from '../ui/Skeleton';
import { IconCopyOutline16 } from '../ui/icons';
import type { StatusTone } from '../vocabulary/status';
import { blockedStates, requestStates } from '../vocabulary/states';
import { failureRemediation } from '../vocabulary/errors';
import { publicCopy } from '../vocabulary/needs';
import { type AppRouter } from '../routing/routes';
import type { PublicClient } from './client';
import { ReviewCard } from './ReviewCard';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import type { ConnectionTarget } from '../connections/client';
import { useRecordedProgress } from './useRecordedProgress';
import { createIntent } from './intent';
import { LaunchProgress } from './LaunchProgress';

export function Progress({
  client,
  id,
  owner,
  onNavigate,
}: {
  readonly client: PublicClient;
  readonly id: string;
  readonly owner?: string;
  readonly onNavigate: AppRouter['navigate'];
}) {
  const progress = useRecordedProgress(client, id);
  const record = progress.record;
  const [preview, setPreview] = useState<TriggerPreview>();
  const [copyStatus, setCopyStatus] = useState<string>();
  useEffect(() => {
    document.title = publicCopy.pageTitle(publicCopy.title);
  }, []);
  const publicId = record?.public_id;
  const draft = record?.state === 'draft';
  const blocked = record?.blocked_reason;
  const agentId =
    record?.resolution.agent.state === 'pending' ? undefined : record?.resolution.agent.id;
  useEffect(() => {
    if ((record?.state === 'ready' || record?.state === 'completed') && agentId !== undefined)
      onNavigate({ kind: 'agent', agentId }, { replace: true });
  }, [record?.state, agentId, onNavigate]);
  useEffect(() => {
    if (publicId === undefined || !draft) return;
    const controller = new AbortController();
    void client.requests.preview(publicId, { signal: controller.signal }).then(
      (value) => {
        if (!controller.signal.aborted) setPreview(value);
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [client, publicId, draft]);
  const back = (value: Launch) =>
    onNavigate({
      kind: 'public_trigger',
      publicId: value.public_id,
      placementId: value.placement_id,
    });
  const startOver = (value: Launch) => {
    const route = {
      kind: 'public_trigger' as const,
      publicId: value.public_id,
      placementId: value.placement_id,
    };
    try {
      createIntent(route, 'new');
      onNavigate(route);
    } catch {
      setCopyStatus(publicCopy.storageUnavailable);
    }
  };
  if (record === undefined)
    return (
      <>
        <h1>{publicCopy.title}</h1>
        {progress.error === undefined ? (
          <SkeletonRows label={publicCopy.loading} />
        ) : (
          <Alert
            tone="error"
            actions={<Button onClick={progress.refresh}>{publicCopy.retry}</Button>}
          >
            {progress.error}
          </Alert>
        )}
      </>
    );
  const summary = blocked === undefined ? requestStates[record.state] : blockedStates[blocked.type];
  const tone = summaryTone(record);
  const connectionTarget: ConnectionTarget = { type: 'launch', id: record.id };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(record.id);
      setCopyStatus(publicCopy.copied);
    } catch {
      setCopyStatus(publicCopy.copyFailed);
    }
  };
  return (
    <section className="public-progress">
      <h1>{publicCopy.title}</h1>
      <Badge tone={tone} live>
        {summary}
      </Badge>
      <LaunchProgress record={record} />
      {progress.observedAt === undefined ? null : (
        <p className="public-observed">
          {progress.error === undefined
            ? publicCopy.observation(new Date(progress.observedAt).toLocaleTimeString())
            : publicCopy.offline(new Date(progress.observedAt).toLocaleTimeString())}
        </p>
      )}
      <Presence>
        {progress.error === undefined ? null : (
          <div className="public-alert">
            <Alert
              tone="warning"
              actions={<Button onClick={progress.refresh}>{publicCopy.checkAgain}</Button>}
            >
              {progress.error}
            </Alert>
          </div>
        )}
      </Presence>
      {blocked?.type === 'connection_required' ? (
        <ConnectionWizard
          presentation="dialog"
          target={connectionTarget}
          onReady={progress.refresh}
          onCancel={() => back(record)}
        />
      ) : null}
      <Presence>
        {record.failure === undefined ? null : (
          <div className="public-alert">
            <Alert
              tone="error"
              actions={
                <>
                  <Button onClick={progress.refresh}>{publicCopy.checkAgain}</Button>
                  {agentId === undefined ? null : (
                    <Button onClick={() => onNavigate({ kind: 'agent', agentId })}>
                      {publicCopy.continue}
                    </Button>
                  )}
                  <Button onClick={() => startOver(record)}>{publicCopy.startOver}</Button>
                  <Button icon={<IconCopyOutline16 />} onClick={() => void copy()}>
                    {publicCopy.copySupport}
                  </Button>
                </>
              }
            >
              {failureRemediation(record.failure.code)}
            </Alert>
            <Presence>
              {copyStatus === undefined ? null : <p role="status">{copyStatus}</p>}
            </Presence>
          </div>
        )}
      </Presence>
      {record.state === 'cancelled' ? (
        <>
          <p>{publicCopy.cancelled}</p>
          <Button onClick={() => startOver(record)}>{publicCopy.startOver}</Button>
        </>
      ) : null}
      <Presence>
        {draft ? (
          <ReviewCard
            key={record.trust.confirmation_digest ?? record.id}
            client={client}
            record={record}
            expectedSubject={owner}
            preview={preview}
            onChanged={progress.changed}
            onClose={() => back(record)}
          />
        ) : null}
      </Presence>
    </section>
  );
}

const stateTones = {
  draft: 'warning',
  resolving: 'active',
  sandbox_pending: 'active',
  workspace_pending: 'active',
  agent_pending: 'active',
  message_pending: 'active',
  ready: 'neutral',
  completed: 'neutral',
  failed_retryable: 'error',
  failed_terminal: 'error',
  cancelled: 'muted',
} as const satisfies Record<Launch['state'], StatusTone>;

const blockedTones = {
  connection_required: 'warning',
  orchestration_paused: 'muted',
  sandbox_paused_reviving: 'active',
  recovery_required: 'warning',
} as const satisfies Record<NonNullable<Launch['blocked_reason']>['type'], StatusTone>;

/** The overall state's tone, from the same record its word comes from. */
function summaryTone(record: Launch): StatusTone {
  const blocked = record.blocked_reason;
  return blocked === undefined ? stateTones[record.state] : blockedTones[blocked.type];
}
