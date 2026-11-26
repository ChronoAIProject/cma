import { deletionTarget, type DeletionRequest } from './deletion';
import { Button } from './ui/Button';
import { Modal } from './ui/Modal';
import { IconCloseOutline16 } from './ui/icons';
import './DeletionNotice.css';

export function DeletionNotice({
  requests,
  disabled,
  onViewStatus,
  onDismiss,
}: {
  readonly requests: readonly DeletionRequest[];
  readonly disabled: boolean;
  readonly onViewStatus: () => void;
  readonly onDismiss: () => void;
}) {
  const unconfirmed = requests.filter((request) => !request.accepted).length;
  const kind = requests[0]?.action.kind;
  const resource = requests.every((request) => request.action.kind === kind)
    ? kind === 'delete_sandbox'
      ? 'environment'
      : kind === 'delete_agent'
        ? 'Agent'
        : 'Workspace'
    : 'resource';
  const count = requests.length;
  if (count === 0) return null;
  return (
    <section className="deletion-summary" aria-label="Deletion request">
      <p role="status">
        {unconfirmed === 0
          ? `Deleting ${count} ${resource}${count === 1 ? '' : 's'}`
          : `${count} deletion request${count === 1 ? '' : 's'} · ${unconfirmed} unconfirmed`}
      </p>
      <div className="deletion-summary__actions">
        <Button size="sm" disabled={disabled} onClick={onViewStatus}>
          View status
        </Button>
        <Button
          size="sm"
          aria-label="Dismiss deletion notice"
          icon={<IconCloseOutline16 size={14} />}
          onClick={onDismiss}
        />
      </div>
    </section>
  );
}

export function DeletionProgress({
  requests,
  onClose,
  onViewStatus,
}: {
  readonly requests: readonly DeletionRequest[];
  readonly onClose: () => void;
  readonly onViewStatus: (request: DeletionRequest) => void;
}) {
  return (
    <Modal open title="Deletion progress" onClose={onClose}>
      <ul className="deletion-summary__requests">
        {requests.map((request, index) => {
          const action = request.action;
          const id =
            action.kind === 'delete_sandbox'
              ? action.sandboxId
              : action.kind === 'delete_agent'
                ? action.agentId
                : action.workspaceId;
          return (
            <li key={request.key}>
              <p>
                {deletionTarget(action)} delete request{' '}
                {request.accepted
                  ? 'accepted. Cleanup continues in the background.'
                  : 'is not confirmed.'}
              </p>
              <code>{id}</code>
              <Button size="sm" onClick={() => onViewStatus(request)}>
                View {deletionTarget(action)} deletion status ({index + 1})
              </Button>
            </li>
          );
        })}
      </ul>
    </Modal>
  );
}
