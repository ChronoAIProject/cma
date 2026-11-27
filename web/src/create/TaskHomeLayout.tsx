import { useContext, useState, type ReactNode } from 'react';
import type { NavigationState } from '../shell/navigation';
import type { RuntimeAttention } from '../shell/runtimeAttention';
import type { AppRoute } from '../routing/routes';
import type { TaskTemplate } from './taskTemplateModel';
import type { AssistantReplyPreview } from '../api/replyPreview';
import { AgentMascot } from '../brand/AgentMascot';
import { DocsLink } from '../services/DocsLink';
import { TaskCrew } from './TaskCrew';
import { selectTaskHomePresentation } from './taskHomePresentation';
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
  const overview = !contextual && !template && !submitted;
  const { empty, crew } = selectTaskHomePresentation({ navigation, contextual });
  const first = overview && empty;
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
        : crew.length
          ? `Good ${hour < 12 ? 'morning' : hour < 18 ? 'afternoon' : 'evening'}.`
          : 'What should your Agent work on?';
  return (
    <section
      className="task-home"
      data-task-home
      data-contextual={contextual || undefined}
      data-first-visit={first || undefined}
      data-overview={(personal && overview) || undefined}
      data-submitted={submitted || undefined}
      aria-labelledby={titleId}
    >
      {submitted ? null : contextual ? (
        <h2 className="sr-only" id={titleId}>
          New task in {contextLabel}
        </h2>
      ) : (
        <header className="task-home__heading">
          {first ? (
            <div className="task-home__first-mascot" aria-hidden="true">
              <div className="task-home__decoration" aria-hidden="true">
                {decoration}
              </div>
              <span className="task-home__first-poster">
                <AgentMascot visual={{ action: 'rest', label: '' }} size={96} decorative />
              </span>
            </div>
          ) : null}
          <h1 id={titleId}>{headline}</h1>
          {template ? (
            <p className="task-home__intro">
              Fill in a few details. Your free-text draft is kept separately.
            </p>
          ) : null}
          <DocsLink view="start-a-task">Help starting a task</DocsLink>
        </header>
      )}
      {overview ? (
        <>
          {personal ? (
            <>
              <HomeRuntimeAttention attention={home?.attention} onReview={home?.onReviewRuntime} />
              {!first ? (
                <TaskCrew
                  navigation={navigation}
                  onRetry={home?.onRetry}
                  onNavigate={onNavigate}
                  previews={previews}
                />
              ) : null}
            </>
          ) : (
            <p className="task-home__context">
              Use the sidebar to browse Agents in the selected scope. New tasks use your personal
              environment.
            </p>
          )}
        </>
      ) : null}
      <div className="task-home__content">{children}</div>
    </section>
  );
}
