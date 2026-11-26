import { useEffect, useState } from 'react';
import { accountGrant } from '../auth/serviceAuthorization';
import { signInPath } from '../auth/nyxid';
import { classifyReturnTarget } from '../auth/returnTarget';
import { Modal } from '../ui/Modal';

const STATUS = {
  authorized: 'NyxID confirms the account grant is active.',
  partial:
    'The account grant is limited to selected services. Other services may need authorization.',
  expired:
    'The account grant has expired. Authorize required services from your draft to continue.',
  not_granted:
    'No current account grant was found. It may not have been granted or may have been revoked.',
  revoked: 'NyxID confirms the account grant was revoked.',
  unknown: 'The account grant status could not be verified.',
  sign_in_required:
    'Your sign-in is no longer valid. Sign in again with the same account to continue.',
} as const;

export function ServiceAuthorizationDialog({
  owner,
  onClose,
}: {
  readonly owner: string;
  readonly onClose: () => void;
}) {
  const [grant, setGrant] = useState<Awaited<ReturnType<typeof accountGrant>>>();
  const destination = classifyReturnTarget(window.location.pathname + window.location.search);
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
      {grant?.managementAccess === 'unsupported_credential' && (
        <p>
          NyxID does not allow CMA to read authorization details with this sign-in. Signing in again
          will not change that access.
        </p>
      )}
      {grant !== undefined &&
        grant.status !== 'unknown' &&
        grant.managementAccess !== 'unsupported_credential' && (
          <a
            href={signInPath(destination.kind === 'reject' ? undefined : destination.target)}
            target="_blank"
            rel="noreferrer"
          >
            Sign in again
          </a>
        )}
      {grant?.managementUrl !== undefined && (
        <a href={grant.managementUrl} target="_blank" rel="noreferrer">
          Manage authorized apps in NyxID
        </a>
      )}
    </Modal>
  );
}
