import { useState } from 'react';
import type { SandboxSummary } from '../api/types';
import {
  sandboxRetirementWaitExceeded,
  type SandboxRetirementObservation,
} from '../sandbox/lifecycle';
import { Button } from '../ui/Button';
import { IconWarningOutline16 } from '../ui/icons/IconWarningOutline16';
import { IconCopyOutline16 } from '../ui/icons/IconCopyOutline16';
import './DeletionProgressList.css';

type DeletingSandbox = Pick<SandboxSummary, 'id' | 'name'> &
  SandboxRetirementObservation & {
    readonly unknown?: boolean;
  };

function DeletionProgressRow({
  sandbox,
  onViewStatus,
}: {
  readonly sandbox: DeletingSandbox;
  readonly onViewStatus: (id: string) => void;
}) {
  const [copy, setCopy] = useState<'copied' | 'failed'>();
  const name = sandbox.name.trim() || 'Sandbox';
  const delayed = !sandbox.unknown && sandboxRetirementWaitExceeded(sandbox);
  return (
    <li className="deletion-progress__row">
      <span className="deletion-progress__name" title={name}>
        {name}
      </span>
      <code className="deletion-progress__identifier" title={sandbox.id}>
        {sandbox.id}
      </code>
      <span className="deletion-progress__state">
        <IconWarningOutline16 />
        {sandbox.unknown
          ? 'Deletion not confirmed'
          : delayed
            ? 'Cleanup taking longer'
            : sandbox.status === 'retiring'
              ? 'Deleting'
              : 'Deletion requested'}
      </span>
      <p className="deletion-progress__note">
        {delayed
          ? 'The expected deletion time has passed. Check status for the latest cleanup progress. '
          : null}
        Still counts toward your environment limit until cleanup is confirmed.
      </p>
      <div className="deletion-progress__actions">
        <Button
          size="sm"
          aria-label={`View status for ${name}`}
          onClick={() => onViewStatus(sandbox.id)}
        >
          View status
        </Button>
        <Button
          size="sm"
          icon={<IconCopyOutline16 />}
          aria-label={`Copy identifier for ${name}`}
          title="Copy identifier"
          onClick={() => {
            void Promise.resolve()
              .then(() => navigator.clipboard.writeText(sandbox.id))
              .then(
                () => setCopy('copied'),
                () => setCopy('failed'),
              );
          }}
        />
      </div>
      {copy === undefined ? null : (
        <p className="deletion-progress__feedback" role="status">
          {copy === 'copied'
            ? 'Identifier copied.'
            : 'Copy unavailable. Select the full identifier below.'}
        </p>
      )}
      {copy === 'failed' ? <code className="deletion-progress__full">{sandbox.id}</code> : null}
    </li>
  );
}

export function DeletionProgressList({
  footer = false,
  sandboxes,
  onViewStatus,
}: {
  readonly footer?: boolean;
  readonly sandboxes: readonly DeletingSandbox[];
  readonly onViewStatus: (id: string) => void;
}) {
  if (sandboxes.length === 0) return null;
  return (
    <details className="deletion-progress" data-navigation-footer={footer || undefined}>
      <summary>
        {sandboxes.some((sandbox) => sandbox.unknown || sandboxRetirementWaitExceeded(sandbox))
          ? `${sandboxes.length} deletions need checking`
          : `Cleaning up ${sandboxes.length} sandboxes`}
      </summary>
      <ul className="deletion-progress__list" aria-label="Sandbox deletion progress">
        {sandboxes.map((sandbox) => (
          <DeletionProgressRow key={sandbox.id} sandbox={sandbox} onViewStatus={onViewStatus} />
        ))}
      </ul>
    </details>
  );
}
