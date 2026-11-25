import type { ReactNode } from 'react';
import { CharacterAvatar } from '../character/CharacterAvatar';
import { Badge } from '../ui/Badge';
import { activityLabel } from '../shell/treePresentation';
import type { PreparationPresentation } from './preparationPresentation';
import { PreparationMessage } from './PreparationMessage';
import './PreparationView.css';

function EvidenceTime({ value }: { readonly value: string }) {
  const absolute = new Date(value).toLocaleString();
  const relative = activityLabel(value);
  return (
    <time dateTime={value} title={absolute} aria-label={absolute} tabIndex={0}>
      {relative === 'Now' ? 'Just now' : `${relative} ago`}
    </time>
  );
}

export function PreparationView({
  model,
  message,
  details,
  actions,
  notices,
  draft,
  titleId,
}: {
  readonly model: PreparationPresentation;
  readonly message?: string;
  readonly details?: ReactNode;
  readonly actions?: ReactNode;
  readonly notices?: ReactNode;
  readonly draft?: ReactNode;
  readonly titleId?: string;
}) {
  if (model.started) return null;
  return (
    <section className="preparation" aria-label="Task preparation">
      <header className="preparation__header">
        <h1 id={titleId}>{model.title}</h1>
        <Badge tone={model.stage === 'failed' ? 'warning' : 'neutral'} wrap live>
          {model.label}
        </Badge>
      </header>
      {message !== undefined ? <PreparationMessage text={message} /> : null}
      <div className="preparation__hero">
        <CharacterAvatar
          pose={model.stage === 'failed' ? 'failed' : 'preparing'}
          size={88}
          still={!model.animate}
        />
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
              <EvidenceTime value={model.acceptedAt} />
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
              Last progress <EvidenceTime value={model.progressAt} />
            </span>
          ) : null}
        </li>
        <li data-state="pending">
          <strong>
            {model.stage === 'failed' ? 'Task hasn’t started' : 'Starts on your message'}
          </strong>
        </li>
      </ol>
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
