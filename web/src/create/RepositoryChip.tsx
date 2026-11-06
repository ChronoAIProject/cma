import { useId, useState } from 'react';
import { Button } from '../ui/Button';
import {
  IconBranchOutline16,
  IconCloseOutline16,
  IconGithub16,
  IconRepositoryOutline16,
} from '../ui/icons';
import { RepositoryDialog } from './RepositoryDialog';

export function RepositoryChip({
  url,
  reference,
  unavailableReason,
  checking = false,
  onChange,
}: {
  readonly url: string;
  readonly reference: string;
  readonly unavailableReason?: string;
  readonly checking?: boolean;
  readonly onChange: (url: string, reference: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const reasonId = useId();
  const triggerId = `${reasonId}-trigger`;
  const focusTrigger = () =>
    requestAnimationFrame(() => document.getElementById(triggerId)?.focus());
  const name = url
    .replace(/^https:\/\/github\.com\//, '')
    .replace(/\/$/, '')
    .replace(/\.git$/, '');
  return (
    <>
      {url || reference ? (
        <div className="task-home__repo">
          <Button
            id={triggerId}
            placement="task-home__repo-name"
            icon={<IconGithub16 />}
            disabled={Boolean(unavailableReason)}
            aria-describedby={unavailableReason ? reasonId : undefined}
            onClick={() => setEditing(true)}
          >
            <span title={url}>{name || 'Repository'}</span>
          </Button>
          <span className="task-home__ref" title={reference || 'default'}>
            <IconBranchOutline16 />
            {reference || 'default'}
          </span>
          <Button
            placement="task-home__remove"
            icon={<IconCloseOutline16 />}
            aria-label="Remove repository"
            onClick={() => {
              onChange('', '');
              focusTrigger();
            }}
          />
        </div>
      ) : (
        <Button
          id={triggerId}
          placement="task-home__tool"
          icon={<IconRepositoryOutline16 />}
          disabled={Boolean(unavailableReason)}
          aria-describedby={unavailableReason ? reasonId : undefined}
          onClick={() => setEditing(true)}
        >
          Add git repo
        </Button>
      )}
      {unavailableReason ? (
        <p className={checking ? 'sr-only' : 'task-home__repository-reason'} id={reasonId}>
          {unavailableReason}
        </p>
      ) : null}
      {editing && !unavailableReason ? (
        <RepositoryDialog
          url={url}
          reference={reference}
          onSave={(url, reference) => {
            onChange(url, reference);
            focusTrigger();
          }}
          onClose={() => setEditing(false)}
        />
      ) : null}
    </>
  );
}
