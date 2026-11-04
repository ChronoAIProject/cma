import { memo } from 'react';
import type { PlanSnapshot } from './model';
import { planAnchor, reportedPlanProgress } from './plans';

export const PlanProgress = memo(function PlanProgress({
  plans,
  activeRunId,
}: {
  readonly plans: readonly PlanSnapshot[];
  readonly activeRunId?: string;
}) {
  const plan =
    activeRunId === undefined ? undefined : plans.find((item) => item.runId === activeRunId);
  if (plan === undefined || (plan.entries.length === 0 && !plan.incomplete)) return null;
  const progress = reportedPlanProgress(plan);
  const label = progress.known
    ? progress.currentIndex < 0
      ? `Working · ${progress.completed} of ${plan.entries.length} done`
      : `Working · step ${progress.currentIndex + 1} of ${plan.entries.length}`
    : 'Progress unknown';
  return (
    <a
      className="agent-toolbar__progress"
      href={`#${planAnchor(plan)}`}
      aria-label={`Current plan: ${label}`}
    >
      {label}
    </a>
  );
});
