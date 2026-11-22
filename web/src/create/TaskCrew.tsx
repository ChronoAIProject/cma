import type { NavigationState } from '../shell/navigation';
import type { AppRoute } from '../routing/routes';
import { taskPresentation } from '../shell/taskPresentation';
import { taskTitle } from '../shell/taskTitle';
import { taskActivityOrder, taskColor } from '../shell/taskGroups';
import { activityLabel } from '../shell/treePresentation';
import { CharacterAvatar, characterPhase } from '../character/CharacterAvatar';
import { characterForStatus } from '../character/status';
import { sandboxChildStatus } from '../sandbox/lifecycle';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

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

export function TaskCrew({
  navigation,
  onNavigate,
  onRetry,
}: {
  readonly navigation?: NavigationState;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onRetry?: () => void;
}) {
  if (!navigation) return null;
  if (navigation.status === 'loading') return <p role="status">Loading your crew…</p>;
  if (navigation.status === 'error')
    return (
      <div className="task-crew__notice" role="alert">
        <p>{navigation.message}</p>
        {onRetry ? <Button onClick={onRetry}>Try again</Button> : null}
      </div>
    );
  const cards = taskCrewCards(navigation);
  return (
    <section className="task-crew" aria-label="Your crew">
      {cards.length ? <h2>Your crew</h2> : null}
      {navigation.refreshError ? <p role="status">{navigation.refreshError}</p> : null}
      <div className="task-crew__cards">
        {cards.slice(0, 3).map(({ agent, sandbox, presentation }) => (
          <button
            type="button"
            key={agent.id}
            className="task-crew__card"
            onClick={() => onNavigate({ kind: 'agent', agentId: agent.id })}
          >
            <span className="task-crew__top">
              <CharacterAvatar
                {...characterForStatus(sandboxChildStatus(sandbox, agent.status))}
                color={sandbox.color ?? taskColor(agent.id)}
                phase={characterPhase(agent.id)}
                size={56}
                still={!characterForStatus(sandboxChildStatus(sandbox, agent.status)).animate}
              />
              <Badge
                tone={
                  navigation.refreshError || presentation.parentNotice
                    ? 'warning'
                    : presentation.status === 'working'
                      ? 'active'
                      : 'neutral'
                }
                wrap
              >
                {presentation.label}
              </Badge>
            </span>
            <strong>{taskTitle(agent)}</strong>
            <span className="task-crew__description">
              {presentation.parentNotice ??
                (navigation.refreshError
                  ? 'Open the Agent to check its current status.'
                  : presentation.status === 'working'
                    ? 'The Agent is working on your task.'
                    : presentation.status === 'not_started'
                      ? 'Ready for your next message.'
                      : presentation.status === 'paused'
                        ? 'Paused. Open the Agent to continue.'
                        : 'Open the conversation for details.')}
            </span>
            <span className="task-crew__meta">
              <span>{sandbox.name}</span>
              {Number.isFinite(Date.parse(agent.updatedAt)) ? (
                <time dateTime={agent.updatedAt}>
                  Updated{' '}
                  {activityLabel(agent.updatedAt) === 'Now'
                    ? 'just now'
                    : `${activityLabel(agent.updatedAt)} ago`}
                </time>
              ) : null}
            </span>
          </button>
        ))}
      </div>
    </section>
  );
}
