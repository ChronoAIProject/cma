import type { AgentController } from './controller';
import type { UploadAttachment } from './attachments';
import type { MessageUser } from './MessageIdentity';
import type { queuePresentation } from './queuePresentation';
import { QueuedMessages, UnappliedQueuedEdits } from './QueuedMessages';
import { QueuedInputOutcomes } from './QueuedInputOutcomes';

export function AgentInputQueue({
  controller,
  queue,
  readOnly,
  controlReady,
  verified,
  draftBlocked,
  onRestore,
  uploadAttachment,
  user,
}: {
  readonly controller: AgentController;
  readonly queue: ReturnType<typeof queuePresentation>;
  readonly readOnly: boolean;
  readonly controlReady: boolean;
  readonly verified: boolean;
  readonly draftBlocked?: boolean;
  readonly onRestore: (text: string) => void;
  readonly uploadAttachment?: UploadAttachment;
  readonly user?: MessageUser;
}) {
  const inputs = controller.queuedInputs;
  if (!inputs) return null;
  return (
    <>
      <QueuedInputOutcomes
        items={queue.outcomes}
        onRefresh={inputs.refresh}
        error={inputs.error}
        onRestore={readOnly ? undefined : onRestore}
        restoreDisabled={
          !verified ||
          !controller.inputAvailability.allowed ||
          draftBlocked ||
          controller.pending !== undefined ||
          controller.pendingMessage !== undefined ||
          controller.pendingControl === true
        }
      />
      <QueuedMessages
        items={queue.items}
        disabled={readOnly || !controlReady || !controller.inputAvailability.allowed}
        onEdit={inputs.edit}
        onRemove={inputs.remove}
        onRecover={inputs.recover}
        uploadAttachment={uploadAttachment}
        user={user}
      />
      {verified && !readOnly && inputs.unappliedEdits ? (
        <UnappliedQueuedEdits
          drafts={inputs.unappliedEdits}
          onDismiss={inputs.dismissUnappliedEdit}
        />
      ) : null}
    </>
  );
}
