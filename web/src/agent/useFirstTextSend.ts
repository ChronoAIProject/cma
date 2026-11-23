import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { isConnectionReport, type ConnectionReport } from '../connections/client';
import type { useConnectionGate } from '../connections/useConnectionGate';
import type { AgentController, AgentResourceState } from './controllerTypes';
import type { useAgentConnectionReadiness } from './connectionReadiness';

export interface FirstTextSend {
  readonly available: boolean;
  readonly pending: boolean;
  readonly cancel: () => void;
  readonly start: (draftIsCurrent: () => boolean, send: (report: ConnectionReport) => void) => void;
}

export function useFirstTextSend({
  state,
  controller,
  client,
  agentId,
  owner,
  authEpoch,
  eligibleView,
  connection,
  connections,
}: {
  readonly state: AgentResourceState;
  readonly controller: AgentController;
  readonly client: object | undefined;
  readonly agentId: string;
  readonly owner: string | undefined;
  readonly authEpoch: number;
  readonly eligibleView: boolean;
  readonly connection: ReturnType<typeof useAgentConnectionReadiness>;
  readonly connections: ReturnType<typeof useConnectionGate>;
}): FirstTextSend {
  const v2 = state.status === 'ready' ? state.v2 : undefined;
  const binding = useMemo(
    () => ({ v2, authEpoch, scope: connection.scope }),
    [v2, authEpoch, connection.scope],
  );
  const [used, setUsed] = useState<typeof binding>();
  const [pending, setPending] = useState(false);
  const intent = useRef<{ binding: typeof binding } | undefined>(undefined);
  const current = useRef<typeof binding | undefined>(undefined);
  const cancelGate = connections.cancel;
  const cancel = useCallback(() => {
    if (intent.current) cancelGate();
  }, [cancelGate]);
  const available =
    import.meta.env.VITE_FIRST_TEXT_SEND_INTENT_ENABLED === 'true' &&
    eligibleView &&
    used !== binding &&
    client !== undefined &&
    owner !== undefined &&
    state.status === 'ready' &&
    state.detail.status === 'ready' &&
    v2?.owner === owner &&
    !state.historyLoading &&
    !state.historyUnavailable &&
    !state.conversation.activeRunId &&
    controller.resourceAvailability.state === 'live' &&
    controller.inputAvailability.allowed &&
    controller.pending === undefined &&
    controller.pendingMessage === undefined &&
    !controller.pendingControl &&
    controller.controlBaselineReady !== false;

  useLayoutEffect(() => {
    current.current = available ? binding : undefined;
    if (intent.current && current.current !== intent.current.binding) cancel();
  }, [available, binding, cancel]);
  useEffect(() => {
    if (!pending) return;
    const hidden = () => {
      if (document.hidden) cancel();
    };
    document.addEventListener('visibilitychange', hidden);
    return () => document.removeEventListener('visibilitychange', hidden);
  }, [pending, cancel]);

  return {
    available,
    pending,
    cancel,
    start(draftIsCurrent, send) {
      if (
        client === undefined ||
        owner === undefined ||
        document.hidden ||
        intent.current ||
        current.current !== binding
      )
        return;
      const action = { binding };
      intent.current = action;
      setPending(true);
      const settle = () => {
        if (intent.current !== action) return false;
        intent.current = undefined;
        setPending(false);
        return true;
      };
      void connections.checkAndRun(
        { type: 'agent', id: agentId },
        owner,
        (report) => {
          if (
            !settle() ||
            current.current !== binding ||
            document.hidden ||
            !draftIsCurrent() ||
            !isConnectionReport(report) ||
            !report.ready ||
            report.owner !== owner
          )
            return;
          current.current = undefined;
          setUsed(binding);
          connection.accept(report);
          send(report);
        },
        undefined,
        settle,
        client,
      );
    },
  };
}
