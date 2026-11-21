import type { TaskAcceptance } from '@cma/client';

export function acceptanceForTurn(task: TaskAcceptance | undefined, turnId: string | undefined) {
  return task && String(task.turn_sequence) === turnId ? task : undefined;
}

export function taskAcceptanceStatus(task: TaskAcceptance | undefined) {
  switch (task?.verdict.status) {
    case 'succeeded':
      return 'task_accepted';
    case 'needs_attention':
    case 'indeterminate':
      return 'needs_attention';
  }
}

export function taskAcceptanceLabel(task: TaskAcceptance | undefined) {
  const status = taskAcceptanceStatus(task);
  if (status === 'task_accepted') return 'Task complete';
  if (status === 'needs_attention') return 'Needs attention';
}

export function taskAcceptanceReason(task: TaskAcceptance | undefined) {
  const verdict = task?.verdict;
  if (!verdict || !('reason' in verdict)) return undefined;
  return {
    required_steps_incomplete: 'Required plan steps are unfinished.',
    required_deliverables_missing: 'Required outputs still need verification.',
    interrupted: 'The required work was interrupted.',
    execution_failed: 'The turn failed before acceptance.',
    history_incomplete: 'Part of the turn history is missing.',
  }[verdict.reason];
}
