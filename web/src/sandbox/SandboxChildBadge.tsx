import type { AgentSummary, SandboxSummary, WorkspaceStatus } from '../api/types';
import { Badge, StatusBadge } from '../ui/Badge';
import { statusTone } from '../vocabulary/status';
import { taskPresentation } from '../shell/taskPresentation';
import { sandboxChildStatus } from './lifecycle';
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';
import { sandboxStatusPresentation } from './presentation';

export function SandboxChildBadge({
  sandbox,
  status,
  agent,
  live = false,
}: {
  readonly sandbox: SandboxSummary;
  readonly status: AgentSummary['status'] | WorkspaceStatus;
  readonly agent?: AgentSummary;
  readonly live?: boolean;
}) {
  if (
    !sandboxHasParkedRuntimeUpdate(sandbox) ||
    ['stopped', 'failed', 'deleted', 'removed', 'removing'].includes(status)
  )
    return <StatusBadge status={sandboxChildStatus(sandbox, status)} live={live} />;
  const task = agent === undefined ? undefined : taskPresentation(agent, sandbox, 'fresh');
  const presentation = sandboxStatusPresentation(sandbox);
  return (
    <Badge
      tone={task !== undefined && task.group === undefined ? statusTone(status) : presentation.tone}
      live={live}
    >
      {task?.label ?? presentation.label}
    </Badge>
  );
}
