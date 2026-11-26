import type { NavigationState } from '../shell/navigation';
import { taskPresentation } from '../shell/taskPresentation';
import { taskActivityOrder } from '../shell/taskGroups';

export function taskCrewCards(navigation?: NavigationState) {
  if (navigation?.status !== 'ready') return [];
  return navigation.items
    .flatMap(({ sandbox, workspaces }) =>
      workspaces
        .filter((workspace) => workspace.status !== 'removed')
        .flatMap((workspace) =>
          workspace.agents.flatMap((agent) => {
            const presentation = taskPresentation(
              agent,
              sandbox,
              navigation.refreshError ? 'stale' : 'fresh',
            );
            return presentation ? [{ agent, sandbox, presentation }] : [];
          }),
        ),
    )
    .sort(
      (a, b) =>
        taskActivityOrder(b.agent) - taskActivityOrder(a.agent) ||
        a.agent.id.localeCompare(b.agent.id),
    );
}

export function selectTaskHomePresentation({
  navigation,
  scope,
  contextual = false,
  templateSelected = false,
  submitted = false,
}: {
  readonly navigation?: NavigationState;
  readonly scope?: string;
  readonly contextual?: boolean;
  readonly templateSelected?: boolean;
  readonly submitted?: boolean;
}) {
  const personal = !scope && !contextual;
  const crew = personal ? taskCrewCards(navigation).slice(0, 3) : [];
  const empty =
    personal && navigation?.status === 'ready' && !navigation.refreshError && crew.length === 0;
  const first = crew[0]?.presentation;
  const commonStatus =
    first &&
    !first.parentNotice &&
    !first.group &&
    !['failed', 'needs_attention', 'waiting_approval', 'unknown'].includes(first.status) &&
    crew.every(
      ({ presentation }) =>
        presentation.status === first.status &&
        presentation.label === first.label &&
        !presentation.parentNotice &&
        !presentation.group,
    )
      ? first.label
      : undefined;
  return {
    crew,
    empty,
    commonStatus,
    templates: submitted ? 'hidden' : empty && !templateSelected ? 'expanded' : 'menu',
  } as const;
}
