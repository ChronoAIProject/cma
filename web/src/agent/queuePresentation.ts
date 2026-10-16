import type { ConversationState } from './model';
import type { OptimisticMessage } from './optimisticMessages';
import type { QueuedInputsControl } from './transport/queuedInputsStatus';

/** Dispatch bookkeeping is not evidence that an idle Send waited behind another turn. */
export function queuePresentation(
  conversation: ConversationState,
  messages: readonly OptimisticMessage[],
  queue: QueuedInputsControl | undefined,
) {
  const command = (message: OptimisticMessage) =>
    message.receipt && 'commandId' in message.receipt ? message.receipt.commandId : undefined;
  const active =
    conversation.activeRunId === String(conversation.responseBinding?.turnSequence)
      ? conversation.responseBinding?.commandId
      : undefined;
  const delivered = new Set(
    conversation.messages
      .filter((message) => message.role === 'user')
      .map((message) => message.commandId),
  );
  const rows = queue?.items ?? [];
  const recovering = new Set(
    rows.filter((item) => item.needsRecovery).map((item) => item.commandId),
  );
  const ordinary = new Set(
    messages
      // Missing metadata belongs to recovered input: retain its existing queue/recovery UI.
      .filter((message) => message.queued === false && message.source === undefined)
      .map(command),
  );
  // Discovery may beat the Send receipt. Hold only newly discovered rows until
  // that ordinary request settles; never correlate different inputs by their text.
  const pendingOrdinary = messages.find(
    (message) =>
      message.queued === false &&
      message.source === undefined &&
      (message.state === 'sending' || message.state === 'unconfirmed') &&
      command(message) === undefined &&
      message.queueBeforeSend !== undefined,
  );
  const awaitingReceipt = pendingOrdinary?.state === 'sending' ? pendingOrdinary : undefined;
  const items = rows.filter(
    (item) =>
      item.needsRecovery ||
      ((awaitingReceipt === undefined ||
        awaitingReceipt.queueBeforeSend!.includes(item.commandId)) &&
        !ordinary.has(item.commandId) &&
        item.commandId !== active &&
        !delivered.has(item.commandId)),
  );
  const normal = new Set(
    rows
      .filter((item) => !item.needsRecovery && ordinary.has(item.commandId))
      .map((item) => item.commandId),
  );
  const visible = messages
    .filter((message) => {
      const id = command(message);
      return (
        id === undefined ||
        !queue?.managedCommandIds.has(id) ||
        normal.has(id) ||
        (id === active && !recovering.has(id))
      );
    })
    .map((message) =>
      command(message) !== undefined && command(message) === active
        ? { ...message, queued: false }
        : message,
    );
  // A restored queue entry can start before its durable user-message frame arrives.
  const starting = rows.find((item) => item.commandId === active && !item.needsRecovery);
  if (
    starting &&
    (pendingOrdinary === undefined ||
      pendingOrdinary.queueBeforeSend!.includes(starting.commandId)) &&
    !delivered.has(active) &&
    !visible.some((message) => command(message) === active)
  )
    visible.push({
      id: `starting:${starting.commandId}`,
      content: starting.input,
      state: 'accepted',
      queued: false,
      receipt: { commandId: starting.commandId, responseId: starting.responseId },
    });
  return { items, messages: visible };
}
