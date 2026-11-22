import { useEffect, useState } from 'react';
import { accountGrant } from '../auth/serviceAuthorization';
import { Modal } from '../ui/Modal';

const STATUS = {
  authorized: 'NyxID confirms the account grant is active.',
  revoked: 'NyxID confirms the account grant was revoked.',
  unknown: 'The account grant status could not be verified.',
} as const;

export function ServiceAuthorizationDialog({
  owner,
  onClose,
}: {
  readonly owner: string;
  readonly onClose: () => void;
}) {
  const [grant, setGrant] = useState<Awaited<ReturnType<typeof accountGrant>>>();
  useEffect(() => {
    const controller = new AbortController();
    void accountGrant(owner, controller.signal).then(
      (result) => {
        if (!controller.signal.aborted) setGrant(result);
      },
      () => {
        if (!controller.signal.aborted) setGrant({ status: 'unknown' });
      },
    );
    return () => controller.abort();
  }, [owner]);

  return (
    <Modal open title="Service authorization" onClose={onClose}>
      <p role="status">{grant === undefined ? 'Checking account grant…' : STATUS[grant.status]}</p>
      {grant?.managementUrl !== undefined && (
        <a href={grant.managementUrl} target="_blank" rel="noreferrer">
          Manage authorized apps in NyxID
        </a>
      )}
    </Modal>
  );
}
