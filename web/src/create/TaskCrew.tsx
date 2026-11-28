import type { NavigationState } from '../shell/navigation';
import type { AppRoute } from '../routing/routes';
import type { AssistantReplyPreview } from '../api/replyPreview';
import { taskTitle } from '../shell/taskTitle';
import { taskColor } from '../shell/taskGroups';
import { activityLabel, agentListAction, agentSignal } from '../shell/treePresentation';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { selectTaskHomePresentation } from './taskHomePresentation';
export { taskCrewCards } from './taskHomePresentation';

export function TaskCrew({
  navigation,
  onNavigate,
  onRetry,
  previews = {},
}: {
  readonly navigation?: NavigationState;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onRetry?: () => void;
  readonly previews?: Readonly<Record<string, AssistantReplyPreview>>;
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
  const { crew: cards, commonStatus } = selectTaskHomePresentation({ navigation });
  return (
    <section className="task-crew" aria-label="Your crew">
      {cards.length ? (
        <div className="task-crew__heading">
          <h2>Your crew</h2>
          {commonStatus ? <span>{commonStatus}</span> : null}
        </div>
      ) : null}
      {navigation.refreshError ? <p role="status">{navigation.refreshError}</p> : null}
      <div className="task-crew__cards">
        {cards.map(({ agent, sandbox, presentation }) => (
          <button
            type="button"
            key={agent.id}
            className="task-crew__card"
            aria-label={`${taskTitle(agent)}, ${presentation.label}${presentation.parentNotice ? `, ${presentation.parentNotice}` : ''}`}
            onClick={() => onNavigate({ kind: 'agent', agentId: agent.id })}
          >
            <span className="task-crew__top">
              {agentSignal(
                agentListAction(agent.status, sandbox, presentation),
                presentation.label,
                48,
                sandbox.color ?? taskColor(agent.id),
              )}
              {!commonStatus ? (
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
              ) : null}
            </span>
            <strong>{taskTitle(agent)}</strong>
            {presentation.parentNotice ? (
              <span className="task-crew__notice">{presentation.parentNotice}</span>
            ) : null}
            {!navigation.refreshError && previews[agent.id] ? (
              <>
                <span className="task-crew__description">{previews[agent.id]!.text}</span>
                <span className="task-crew__meta">
                  <time dateTime={previews[agent.id]!.appendedAt}>
                    Replied{' '}
                    {activityLabel(previews[agent.id]!.appendedAt) === 'Now'
                      ? 'just now'
                      : `${activityLabel(previews[agent.id]!.appendedAt)} ago`}
                  </time>
                </span>
              </>
            ) : null}
          </button>
        ))}
      </div>
    </section>
  );
}
