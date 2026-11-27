import type { TaskAcceptance } from '@cma/client';
import type { AgentStatus, SandboxStatus } from '../api/types';
import { currentExternalApproval } from '../agent/externalApproval';
import type { ConversationState } from '../agent/model';
import { recordedTurnActivity } from '../agent/ThinkingProgress';
import { acceptanceForTurn, taskAcceptanceReason } from '../agent/taskAcceptance';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import { sandboxStatusPresentation, type SandboxStatusObservation } from '../sandbox/presentation';
import { taskWords, type TaskStatus } from '../shell/taskStatus';

export type AgentVisualAction =
  'rest' | 'ready' | 'idea' | 'working' | 'question' | 'love' | 'preparing' | 'failed';

export interface AgentVisual {
  readonly action: AgentVisualAction;
  readonly label: string;
  readonly canAnimate: boolean;
  readonly detail?: string;
}

function visual(action: AgentVisualAction, label: string, canAnimate = false): AgentVisual {
  return { action, label, canAnimate };
}

const lifecycle = {
  starting: visual('preparing', taskWords.starting, true),
  ready: visual('ready', 'Ready'),
  working: visual('working', taskWords.working, true),
  waiting_approval: visual('question', taskWords.waiting_approval, true),
  paused: visual('rest', taskWords.paused),
  stopped: visual('rest', 'Agent stopped'),
  deleted: visual('rest', 'Agent deleted'),
  failed: visual('failed', 'Agent failed', true),
} satisfies Record<AgentStatus, AgentVisual>;

const parentActions = {
  provisioning: 'preparing',
  installing: 'preparing',
  pausing: 'rest',
  paused: 'rest',
  reviving: 'preparing',
  rotating: 'preparing',
  failed: 'failed',
  retiring: 'rest',
  retired: 'rest',
} satisfies Record<Exclude<SandboxStatus, 'active' | 'credential_rotating'>, AgentVisualAction>;

const loading = visual('preparing', taskWords.loading, true);
const unknown = visual('rest', taskWords.unknown);
const turnEnded = visual('ready', taskWords.completed);
const taskVisuals: Partial<Record<TaskStatus, AgentVisual>> = {
  loading,
  not_started: visual('ready', taskWords.not_started),
  completed: turnEnded,
  working: lifecycle.working,
  waiting_approval: lifecycle.waiting_approval,
  interrupted: visual('rest', taskWords.interrupted),
  failed: visual('failed', 'This turn failed', true),
  unknown,
};

interface TurnObservation {
  readonly conversation: ConversationState;
  readonly observedAt: number;
}

interface AgentVisualInput {
  /** Current authorized resource status and task status selected by the caller. */
  readonly status?: AgentStatus;
  readonly task?: TaskStatus;
  readonly sandbox?: SandboxStatusObservation;
  readonly acceptance?: TaskAcceptance;
  readonly turnId?: string;
  readonly activity?: TurnObservation;
}

function activeTurn(conversation: ConversationState): string | undefined {
  const { activeRunId, latestTurnId, turnOutcomes } = conversation;
  if (
    activeRunId !== undefined &&
    (latestTurnId === undefined || latestTurnId === activeRunId) &&
    turnOutcomes?.[activeRunId] === undefined
  )
    return activeRunId;
}

function liveVisual({ conversation, observedAt }: TurnObservation, runId: string): AgentVisual {
  const waiting = conversation.approvals.some(
    (approval) =>
      approval.source !== 'external' && approval.runId === runId && approval.decision === undefined,
  );
  if (waiting || currentExternalApproval(conversation, observedAt))
    return lifecycle.waiting_approval;
  const { phase } = recordedTurnActivity(conversation);
  return phase === 'Reasoning' ? visual('idea', 'Thinking', true) : visual('working', phase, true);
}

export function agentVisual(input: AgentVisualInput): AgentVisual {
  const { status, task, sandbox, acceptance, turnId, activity } = input;
  if (status === undefined || !Object.hasOwn(lifecycle, status))
    return task === 'loading' ? loading : unknown;

  if (sandbox !== undefined && sandboxChildStatus(sandbox, status) !== status) {
    const parent = sandboxStatusPresentation(sandbox);
    if (parent.group === 'needs_attention') return visual('failed', parent.label);
    if (parent.status !== 'active' && parent.status !== 'credential_rotating') {
      const action = parentActions[parent.status];
      return visual(action, parent.label, action === 'preparing');
    }
  }

  if (status !== 'ready' && status !== 'working' && status !== 'waiting_approval')
    return lifecycle[status];

  if (activity !== undefined) {
    const runId = activeTurn(activity.conversation);
    const working =
      task === 'working' ||
      task === 'waiting_approval' ||
      (task === undefined && status !== 'ready');
    if (runId !== undefined && (turnId === undefined || turnId === runId) && working)
      return liveVisual(activity, runId);
    if (task === 'waiting_approval') return lifecycle[status];
    if (runId !== undefined && (task === 'completed' || task === 'task_accepted'))
      return lifecycle[status];
  }

  const currentAcceptance = acceptanceForTurn(acceptance, turnId);
  if (task === 'task_accepted') {
    if (status !== 'ready') return lifecycle[status];
    const latestTurn = activity?.conversation.latestTurnId;
    return currentAcceptance?.verdict.status === 'succeeded' &&
      (latestTurn === undefined || latestTurn === turnId)
      ? visual('love', taskWords.task_accepted, true)
      : turnEnded;
  }
  if (task === 'needs_attention') {
    const verdict = currentAcceptance?.verdict;
    const failed =
      verdict !== undefined && 'reason' in verdict && verdict.reason === 'execution_failed';
    const detail = taskAcceptanceReason(currentAcceptance);
    const attention = visual(failed ? 'failed' : 'question', taskWords.needs_attention);
    return detail === undefined ? attention : { ...attention, detail };
  }
  if (task === 'not_started' && status !== 'ready') return lifecycle[status];
  return (task === undefined ? undefined : taskVisuals[task]) ?? lifecycle[status];
}
