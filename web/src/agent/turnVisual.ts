import { createContext, useEffect, useMemo, useState } from 'react';
import type { TaskAcceptance } from '@cma/client';
import type { AgentStatus } from '../api/types';
import { agentVisual } from '../brand/agentVisual';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import type { SandboxStatusObservation } from '../sandbox/presentation';
import { currentExternalApproval } from './externalApproval';
import type { ConversationState } from './model';
import { acceptanceForTurn, taskAcceptanceStatus } from './taskAcceptance';

export interface TurnPresentation {
  readonly status: AgentStatus;
  readonly authEpoch: number;
  readonly sandbox?: SandboxStatusObservation;
}

export const TurnMascotContext = createContext(false);
export type TurnVisual = ReturnType<typeof useTurnVisual>;

export function useTurnVisual(
  presentation: TurnPresentation | undefined,
  authority: object | undefined,
  conversation: ConversationState,
  taskAcceptance: TaskAcceptance | undefined,
) {
  const { activeRunId, latestTurnId, agentIncarnation, turnOutcomes } = conversation;
  const turnId = activeRunId ?? latestTurnId;
  const scope = useMemo(
    () => ({ authority, agentIncarnation, turnId }),
    [authority, agentIncarnation, turnId],
  );
  const [observedAt, setObservedAt] = useState(Date.now);
  const pending = authority && presentation && currentExternalApproval(conversation);
  const deadline = pending && currentExternalApproval(conversation, observedAt)?.deadlineAt;
  useEffect(() => {
    if (!pending) return;
    const observe = () => setObservedAt(Date.now());
    observe();
    const delay = Date.parse(deadline ?? '') - Date.now();
    const timer = delay > 0 ? window.setTimeout(observe, Math.min(delay, 2 ** 31 - 1)) : undefined;
    document.addEventListener('visibilitychange', observe);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', observe);
    };
  }, [deadline, pending, scope]);

  if (!authority || !presentation || turnId === undefined) return;
  const { status, sandbox } = presentation;
  if (
    (status !== 'ready' && status !== 'working' && status !== 'waiting_approval') ||
    (sandbox !== undefined && sandboxChildStatus(sandbox, status) !== status)
  )
    return;
  const outcome = turnOutcomes?.[turnId];
  const active =
    activeRunId !== undefined &&
    (latestTurnId === undefined || latestTurnId === turnId) &&
    outcome === undefined;
  const completed = status === 'ready' && activeRunId === undefined && outcome === 'completed';
  if (!active && !completed) return;
  const acceptance = completed ? acceptanceForTurn(taskAcceptance, turnId) : undefined;
  const current = agentVisual({
    status,
    task: active ? 'working' : (taskAcceptanceStatus(acceptance) ?? 'completed'),
    acceptance,
    turnId,
    activity: { conversation, observedAt },
  });
  const visual =
    pending && deadline === undefined && current.action !== 'question'
      ? ({
          action: 'rest',
          label: 'Waiting for the server to confirm the approval outcome',
          canAnimate: false,
        } as const)
      : current;
  return { visual, playback: { scope, enabled: visual.canAnimate } };
}
