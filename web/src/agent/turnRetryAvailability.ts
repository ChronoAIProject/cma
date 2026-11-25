import type { AgentController } from './controllerTypes';
import type { AgentDraft } from './draft';

export function nativeApprovalBusy(controller: AgentController, controlReady: boolean) {
  return (
    controller.pending === 'approval' ||
    !controlReady ||
    !controller.inputAvailability.allowed ||
    controller.controlsUnavailable === true
  );
}

export function turnRetryAvailable(
  controller: AgentController,
  authorized: boolean,
  draft?: AgentDraft,
  actionsBusy = false,
): boolean {
  return (
    authorized &&
    controller.state.status === 'ready' &&
    !controller.state.historyLoading &&
    !controller.state.historyUnavailable &&
    controller.inputAvailability.allowed &&
    controller.controlBaselineReady !== false &&
    !actionsBusy &&
    !draft?.recoveryBlocked &&
    controller.pending === undefined &&
    controller.pendingMessage === undefined &&
    !controller.pendingControl &&
    !controller.checkingInputReceipt &&
    !controller.queuedInputs?.items.length &&
    !controller.queuedInputs?.error &&
    !controller.queuedMessage?.message &&
    !controller.queuedMessage?.needsRecovery &&
    controller.acceptedMessage === undefined &&
    controller.acceptedSteeringMessage === undefined &&
    controller.acceptedInterruptionMessage === undefined
  );
}
