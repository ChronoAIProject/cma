import { Presence } from '../ui/Presence';
import { useEffect, useState } from 'react';
import type { Launch, TriggerPreview } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconCopyOutline16 } from '../ui/icons';
import { BrandLoading, LandingCharacter } from '../landing/LandingCharacter';
import { blockedStates, requestStates, stepStates } from '../vocabulary/states';
import { progressDetail, stepNames, stepOrder } from '../vocabulary/steps';
import { failureRemediation } from '../vocabulary/errors';
import { publicCopy } from '../vocabulary/needs';
import { type AppRouter } from '../routing/routes';
import type { PublicClient } from './client';
import { ReviewCard } from './ReviewCard';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import type { ConnectionTarget } from '../connections/client';
import { useRecordedProgress } from './useRecordedProgress';
import { createIntent } from './intent';
import { LaunchSandboxProgress } from '../sandbox/LaunchSandboxProgress';

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
  const sandboxId =
    record?.resolution.sandbox.state === 'pending' ? undefined : record?.resolution.sandbox.id;
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
          <BrandLoading label={publicCopy.loading} />
        ) : (
          <>
            <p className="public-error" role="alert">
              {progress.error}
            </p>
            <Button onClick={progress.refresh}>{publicCopy.retry}</Button>
          </>
        )}
      </>
    );
  const summary = blocked === undefined ? requestStates[record.state] : blockedStates[blocked.type];
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
      <p role="status" aria-live="polite">
        {summary}
      </p>
      <ol className="public-timeline">
        {stepOrder.map((key) => {
          const step = record.steps.find((item) => item.key === key);
          const state =
            step?.status === 'blocked' && blocked?.type === 'sandbox_paused_reviving'
              ? 'running'
              : (step?.status ?? 'pending');
          const label =
            state === 'blocked' && blocked?.type === 'orchestration_paused'
              ? blockedStates.orchestration_paused
              : stepStates[state];
          const detail = progressDetail(step?.safe_detail);
          return (
            <li
              key={key}
              className={`public-step${state === 'failed' ? ' public-step--failed' : ''}`}
            >
              <span className="public-step__icon" aria-hidden="true">
                <LandingCharacter
                  compact
                  state={
                    state === 'completed'
                      ? 'completed'
                      : state === 'failed'
                        ? 'failed'
                        : state === 'running'
                          ? 'running'
                          : 'pending'
                  }
                />
              </span>
              <div>
                <div className="public-step__line">
                  <h2>{stepNames[key]}</h2>
                  <span>{label}</span>
                </div>
                {detail === undefined ? null : <p>{detail}</p>}
                {key === 'sandbox' &&
                record.state === 'sandbox_pending' &&
                sandboxId !== undefined ? (
                  <LaunchSandboxProgress key={sandboxId} sandboxId={sandboxId} />
                ) : null}
              </div>
            </li>
          );
        })}
      </ol>
      {progress.observedAt === undefined ? null : (
        <p className="public-observed">
          {progress.error === undefined
            ? publicCopy.observation(new Date(progress.observedAt).toLocaleTimeString())
            : publicCopy.offline(new Date(progress.observedAt).toLocaleTimeString())}
        </p>
      )}
      <Presence>
        {progress.error === undefined ? null : (
          <div className="public-notice">
            <p>{progress.error}</p>
            <Button onClick={progress.refresh}>{publicCopy.checkAgain}</Button>
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
          <div className="public-notice">
            <p className="public-error" role="alert">
              {failureRemediation(record.failure.code)}
            </p>
            <div className="public-actions">
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
            </div>
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
