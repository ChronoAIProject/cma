import type { AccountDeletionReceipt } from '@chronoai/cma-sdk';
import { ProvisioningProgress } from '../sandbox/ProvisioningProgress';
import { Button } from '../ui/Button';
import {
  accountDeletionBlockedCopy,
  accountDeletionReadFailureCopy,
  accountDeletionSteps,
} from './deletion';

export function AccountDeletionProgress({
  receipt,
  accepted = true,
  readError,
  onCheckRequest,
  checking = false,
}: {
  readonly receipt?: AccountDeletionReceipt;
  readonly accepted?: boolean;
  readonly readError?: string;
  readonly onCheckRequest: () => void;
  readonly checking?: boolean;
}) {
  const blocked = accountDeletionBlockedCopy(receipt);
  const completed = receipt?.status === 'completed';
  const awaitingAcceptance = accepted === false && receipt === undefined;
  return (
    <>
      <p role="status">
        {completed
          ? 'Cleanup completed and verified.'
          : awaitingAcceptance
            ? 'The deletion request has not been confirmed. Check request to reconcile.'
            : 'Your request was accepted. Cleanup is still in progress.'}
      </p>
      {blocked === undefined ? null : <p role="status">{blocked}</p>}
      {readError === undefined ? null : <p role="alert">{accountDeletionReadFailureCopy()}</p>}
      <details>
        <summary>Cleanup progress</summary>
        <ProvisioningProgress title="Cleanup progress" steps={accountDeletionSteps(receipt)} />
      </details>
      <Button disabled={checking} onClick={onCheckRequest}>
        {checking ? 'Checking request' : 'Check request'}
      </Button>
    </>
  );
}
