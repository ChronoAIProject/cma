import type { ExistingLaunchAgent, LaunchRepeatChoice } from '@cma/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { publicCopy } from '../vocabulary/needs';

export function RepeatChoice({
  existing,
  busy,
  onChoose,
  onClose,
}: {
  readonly existing: ExistingLaunchAgent;
  readonly busy: boolean;
  readonly onChoose: (choice: LaunchRepeatChoice) => void;
  readonly onClose: () => void;
}) {
  return (
    <Modal
      open
      title={publicCopy.repeat}
      onClose={onClose}
      dismissible={!busy}
      footer={
        <>
          <Button disabled={busy} onClick={() => onChoose('new')}>
            {publicCopy.fresh}
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => onChoose('resume')}>
            {publicCopy.previous}
          </Button>
        </>
      }
    >
      <div className="public-review">
        <h3>{existing.title ?? publicCopy.untitled}</h3>
        <time dateTime={existing.created_at}>{new Date(existing.created_at).toLocaleString()}</time>
        <p>{existing.first_message_excerpt}</p>
      </div>
    </Modal>
  );
}
