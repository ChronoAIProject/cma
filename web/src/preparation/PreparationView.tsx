import { useMemo, type ReactNode } from 'react';
import type { CmaWebClient } from '../api/client';
import { AgentMascot } from '../brand/AgentMascot';
import { AgentProvisioningProgress } from '../create/AgentProvisioningProgress';
import { provisioningDuration } from '../create/agentProvisioning';
import { useProvisioningClock } from '../create/useProvisioningClock';
import { Badge } from '../ui/Badge';
import type { PreparationPresentation } from './preparationPresentation';
import { PreparationMessage } from './PreparationMessage';
import './PreparationView.css';

export interface PreparationMotionContext {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly authEpoch?: number;
  readonly visible?: boolean;
}

interface PreparationMotion extends PreparationMotionContext {
  readonly requestKey?: string;
  readonly agentId?: string;
  readonly incarnation?: string;
}

function EvidenceTime({ value, now }: { readonly value: string; readonly now: number }) {
  const absolute = new Date(value).toLocaleString();
  const relative = provisioningDuration(value, now);
  return (
    <time dateTime={value} title={absolute} aria-label={absolute} tabIndex={0}>
      {relative === undefined || relative === '0s' ? 'Just now' : `${relative} ago`}
    </time>
  );
}

export function PreparationView({
  model,
  motion,
  message,
  recordedProgress,
  details,
  actions,
  notices,
  draft,
  titleId,
}: {
  readonly model: PreparationPresentation;
  readonly motion?: PreparationMotion;
  readonly message?: string;
  readonly recordedProgress?: ReactNode;
  readonly details?: ReactNode;
  readonly actions?: ReactNode;
  readonly notices?: ReactNode;
  readonly draft?: ReactNode;
  readonly titleId?: string;
}) {
  const { client, viewerId, authEpoch, requestKey, agentId, incarnation, visible } = motion ?? {};
  const scope = useMemo(
    () => ({ client, viewerId, authEpoch, requestKey, agentId, incarnation }),
    [client, viewerId, authEpoch, requestKey, agentId, incarnation],
  );
  const updating = model.accepted && !model.ready && !model.started && model.stage !== 'failed';
  const now = useProvisioningClock(updating && visible !== false);
  if (model.started) return null;
  return (
    <section className="preparation" aria-label="Task preparation">
      <header className="preparation__header">
        <h1 id={titleId}>{model.title}</h1>
        <Badge tone={model.stage === 'failed' ? 'warning' : 'neutral'} wrap>
          {model.label}
        </Badge>
      </header>
      {message !== undefined ? <PreparationMessage text={message} /> : null}
      <div className="preparation__hero">
        <span inert={visible === false || undefined}>
          <AgentMascot
            visual={model.visual}
            size={48}
            decorative
            playback={{ scope, enabled: model.animate && Boolean(viewerId) }}
          />
        </span>
        <div>
          <h2>{model.headline}</h2>
          {model.notStarted ? (
            <p>It hasn’t started yet. Your message will be its first job.</p>
          ) : null}
          {model.stage === 'ready' ? (
            <p>Checking whether your original message has started.</p>
          ) : null}
          {model.stage === 'checking' ? (
            <p>Keep the original request while its acceptance is checked.</p>
          ) : null}
        </div>
      </div>
      <ol className="preparation__steps" aria-label="Task preparation steps">
        <li data-state={model.accepted ? 'confirmed' : 'unknown'}>
          <strong>{model.accepted ? 'Request accepted' : 'Checking request acceptance'}</strong>
          {model.accepted ? (
            model.acceptedAt ? (
              <EvidenceTime value={model.acceptedAt} now={now} />
            ) : (
              <span>Acceptance time unavailable</span>
            )
          ) : null}
        </li>
        <li
          data-state={model.ready ? 'confirmed' : 'pending'}
          aria-current={
            model.accepted && !model.ready && model.stage !== 'failed' ? 'step' : undefined
          }
        >
          <strong>
            {model.ready
              ? 'Cloud computer ready'
              : model.stage === 'failed'
                ? 'Setup stopped'
                : 'Preparing cloud computer'}
          </strong>
          {model.progressAt ? (
            <span>
              Last progress <EvidenceTime value={model.progressAt} now={now} />
            </span>
          ) : null}
        </li>
        <li data-state="pending">
          <strong>
            {model.stage === 'failed' ? 'Task hasn’t started' : 'Starts on your message'}
          </strong>
        </li>
      </ol>
      {recordedProgress}
      <AgentProvisioningProgress
        receipt={model.receipt}
        operation={model.operation}
        task={{
          now,
          stage: model.stage,
          label: model.label,
          phaseHistoryUnavailable: Boolean(recordedProgress && !model.operation),
        }}
      />
      {model.error ? (
        <p className="preparation__error" role="alert">
          {model.error}
        </p>
      ) : null}
      {notices}
      {actions ? <div className="preparation__actions">{actions}</div> : null}
      {details ? (
        <details className="preparation__details">
          <summary>Setup details</summary>
          {details}
        </details>
      ) : null}
      {draft}
    </section>
  );
}
