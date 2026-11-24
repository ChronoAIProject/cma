import type { CmaWebClient } from './api/client';
import { deletionTarget, type DeleteAction, type DeletionObservation } from './deletion';
import { DeletionImpactSummary } from './DeletionImpactSummary';
import { RiskConfirmation } from './ui/RiskConfirmation';

export function DeletionConfirmation({
  client,
  viewerId,
  action,
  observation,
  enabled,
  readError,
  commandError,
  onCancel,
  onConfirm,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly action: DeleteAction;
  readonly observation: DeletionObservation | undefined;
  readonly enabled: boolean;
  readonly readError: string | undefined;
  readonly commandError: string | undefined;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}) {
  const target = deletionTarget(action);
  return (
    <RiskConfirmation
      open
      title={`Delete ${target}`}
      description={
        <DeletionImpactSummary
          key={JSON.stringify([viewerId, action])}
          client={client}
          viewerId={viewerId}
          action={action}
          observation={observation}
          available={observation !== undefined && readError === undefined}
        />
      }
      acknowledgeLabel={`I understand this ${target} will be deleted.`}
      cancelLabel={action.kind === 'delete_sandbox' ? 'Keep environment' : undefined}
      actionEmphasis={action.kind === 'delete_sandbox' ? 'cancel' : undefined}
      confirmLabel={
        action.kind === 'delete_sandbox'
          ? 'Request deletion'
          : observation?.stopsAgent
            ? 'Stop and delete'
            : `Delete ${target}`
      }
      confirmDisabled={
        !enabled ||
        observation === undefined ||
        readError !== undefined ||
        observation.blocked !== undefined
      }
      status={observation === undefined ? 'Checking deletion availability.' : observation.blocked}
      failure={commandError ?? readError}
      onCancel={onCancel}
      onConfirm={onConfirm}
    />
  );
}
