import { useContext, useState, type ReactNode } from 'react';
import type { NavigationState } from '../shell/navigation';
import type { RuntimeAttention } from '../shell/runtimeAttention';
import type { AppRoute } from '../routing/routes';
import type { TaskTemplate } from './taskTemplateModel';
import type { AssistantReplyPreview } from '../api/replyPreview';
import { CharacterAvatar } from '../character/CharacterAvatar';
import { DocsLink } from '../services/DocsLink';
import { TaskCrew, taskCrewCards } from './TaskCrew';
import { TaskHomeDecorationContext } from './TaskHomeDecoration';
import { HomeRuntimeAttention } from './HomeRuntimeAttention';

export interface TaskHomeOverview {
  readonly navigation?: NavigationState;
  readonly scope?: string;
  readonly onRetry?: () => void;
  readonly onNavigate?: (route: AppRoute) => void;
  readonly onEnvironments?: () => void;
  readonly onReviewRuntime?: () => void;
  readonly attention?: RuntimeAttention;
  readonly authEpoch?: number;
  readonly visible?: boolean;
}

export function TaskHomeLayout({
  titleId,
  home,
  contextual,
  contextLabel,
  template,
  submitted,
  onNavigate,
  previews,
  children,
}: {
  readonly titleId: string;
  readonly home?: TaskHomeOverview;
  readonly contextual: boolean;
  readonly contextLabel?: string;
  readonly template?: TaskTemplate;
  readonly submitted: boolean;
  readonly onNavigate: (route: AppRoute) => void;
  readonly previews?: Readonly<Record<string, AssistantReplyPreview>>;
  readonly children: ReactNode;
}) {
  const decoration = useContext(TaskHomeDecorationContext);
  const [hour] = useState(() => new Date().getHours());
  const personal = !home?.scope;
  const navigation = personal ? home?.navigation : undefined;
  const first =
    navigation?.status === 'ready' &&
    !navigation.refreshError &&
    taskCrewCards(navigation).length === 0;
  const hasCrew = taskCrewCards(navigation).length > 0;
  const templateHeadlines = {
    research: 'What should it research?',
    spreadsheet: 'What should it analyze?',
    document: 'What should it write?',
    website: 'What should it build?',
    code: 'What should it code?',
  };
  const headline = submitted
    ? 'Task request'
    : template
      ? templateHeadlines[template.id]
      : first
        ? 'Give it a job.'
        : hasCrew
          ? `Good ${hour < 12 ? 'morning' : hour < 18 ? 'afternoon' : 'evening'}.`
          : 'What should your Agent work on?';
  return (
    <section
      className="task-home"
      data-task-home
      data-contextual={contextual || undefined}
      data-first-visit={(first && !template && !submitted) || undefined}
      data-submitted={submitted || undefined}
      aria-labelledby={titleId}
    >
      {submitted ? null : contextual ? (
        <h2 className="sr-only" id={titleId}>
          New task in {contextLabel}
        </h2>
      ) : (
        <header className="task-home__heading">
          {first && !template && !submitted ? (
            <>
              <div className="task-home__first-mascot">
                <div className="task-home__decoration" aria-hidden="true">
                  {decoration}
                </div>
                <CharacterAvatar pose="idle" size={104} still />
              </div>
              <span className="task-home__eyebrow">Your next agent starts here</span>
            </>
          ) : null}
          <h1 id={titleId}>{headline}</h1>
          {template || submitted ? (
            <p className="task-home__intro">
              {submitted
                ? 'Your original request and its recorded preparation status.'
                : template
                  ? 'Fill in a few details. Your free-text draft is kept separately.'
                  : hasCrew
                    ? 'Pick up where you left off, or give another agent a job.'
                    : 'Describe what you need. Your Agent works in its own cloud computer.'}
            </p>
          ) : null}
          <DocsLink view="start-a-task">Help starting a task</DocsLink>
        </header>
      )}
      {!contextual && !template && !submitted ? (
        <>
          {personal ? (
            <>
              <HomeRuntimeAttention attention={home?.attention} onReview={home?.onReviewRuntime} />
              <TaskCrew
                navigation={navigation}
                onRetry={home?.onRetry}
                onNavigate={onNavigate}
                previews={previews}
              />
            </>
          ) : (
            <p className="task-home__context">
              Use the sidebar to browse Agents in the selected scope. New tasks use your personal
              environment.
            </p>
          )}
        </>
      ) : null}
      <div
        className="task-home__workspace"
        data-with-mascot={(!contextual && !first && !submitted) || undefined}
      >
        {!contextual && !first && !submitted ? (
          <aside className="task-home__mascot" aria-hidden="true">
            <CharacterAvatar pose="idle" size={80} still />
          </aside>
        ) : null}
        <div className="task-home__content">{children}</div>
      </div>
    </section>
  );
}
