/**
 * How far along a session is while it starts.
 *
 * The control plane names each step and says where it sits in the sequence, so
 * this maps those names to words a person reads. The order lives on the server:
 * a step added there moves the bar here without this file changing, which is
 * what stops the two drifting.
 */

/** Steps the control plane reports, in the order they happen. */
export const STAGES = [
  'accepted',
  'runtime_requested',
  'runtime_created',
  'skills_installed',
  'attaching',
  'ready',
] as const;

export type Stage = (typeof STAGES)[number];

/** What each step means, for someone watching it happen. */
const LABELS: Record<Stage, string> = {
  accepted: 'Request received',
  runtime_requested: 'Asking for a sandbox',
  runtime_created: 'Sandbox created',
  skills_installed: 'Installing skills',
  attaching: 'Connecting to the terminal',
  ready: 'Ready',
};

export function isStage(value: unknown): value is Stage {
  return typeof value === 'string' && (STAGES as readonly string[]).includes(value);
}

export function stageLabel(stage: Stage): string {
  return LABELS[stage];
}

/**
 * How complete the start is, as a fraction.
 *
 * Derived from position in the sequence rather than from elapsed time, so the
 * bar reflects what has actually happened. A bar that advances on a timer looks
 * like progress and tells you nothing.
 */
export function stageProgress(stage: Stage): number {
  const index = STAGES.indexOf(stage);
  return index < 0 ? 0 : (index + 1) / STAGES.length;
}
