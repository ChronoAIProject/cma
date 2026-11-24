import type { AgentV2Session } from './client';
import type { QueuedInputIntent } from './queuedInputJournal';
import type { QueuedCancellationRecord } from './queuedJournal';

export interface PendingRemoval {
  readonly commandId: string;
  readonly responseId: string;
  readonly input?: string;
  readonly resolve: () => boolean;
}

function sameLegacyIntent(
  current: QueuedCancellationRecord | null,
  original: QueuedCancellationRecord,
) {
  return (
    current?.commandId === null &&
    current.key === original.key &&
    current.responseId === original.responseId &&
    current.body.source_command_id === original.body.source_command_id &&
    current.body.expected_command_revision === original.body.expected_command_revision
  );
}

/** Take a snapshot before reading the source so a late reply cannot clear a replacement. */
export function pendingRemovals(
  session: AgentV2Session,
  intents: readonly QueuedInputIntent[],
): PendingRemoval[] {
  const { queuedInputs, queuedCancellation } = session.journal;
  const removals = intents.flatMap((intent): PendingRemoval[] =>
    intent.action !== 'remove'
      ? []
      : [
          {
            commandId: intent.commandId,
            responseId: intent.responseId,
            input: intent.original,
            resolve: () => {
              const current = queuedInputs
                .read()
                .find((entry) => entry.responseId === intent.responseId);
              if (
                current?.key !== intent.key ||
                current.action !== intent.action ||
                current.commandId !== intent.commandId ||
                current.revision !== intent.revision ||
                current.original !== intent.original
              )
                return false;
              queuedInputs.settle(intent);
              return true;
            },
          },
        ],
  );
  const legacy = queuedCancellation.read();
  if (legacy?.commandId === null) {
    const lastInput = session.journal.read();
    removals.push({
      commandId: legacy.body.source_command_id,
      responseId: legacy.responseId,
      input:
        lastInput?.state === 'accepted' &&
        lastInput.receipt.commandId === legacy.body.source_command_id &&
        lastInput.receipt.responseId === legacy.responseId
          ? lastInput.input
          : undefined,
      resolve: () => {
        if (!sameLegacyIntent(queuedCancellation.read(), legacy)) return false;
        queuedCancellation.refused(legacy.key);
        return true;
      },
    });
  }
  return removals;
}
