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
    ? `${progress.completed}/${plan.entries.length} steps${progress.currentIndex < 0 ? '' : ` · Step ${progress.currentIndex + 1}`}`
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
