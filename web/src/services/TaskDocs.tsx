import { useEffect, useRef } from 'react';
import type { HelpTopic } from '../routing/docsRoutes';
import type { AppRoute } from '../routing/routes';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { DocsLink } from './DocsLink';
import { TASK_GUIDES } from './taskHelpContent';
import './TaskDocs.css';

export function TaskDocs({
  view,
  onNavigate,
}: {
  readonly view?: HelpTopic;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const guide = view ? TASK_GUIDES[view] : undefined;
  const title = guide?.title ?? 'Docs';
  useDocumentTitle(title);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    heading.current?.focus();
  }, [view]);
  return (
    <section className="task-docs">
      <PageHeader
        breadcrumb={
          guide
            ? [
                { label: 'Docs', href: '/docs', onSelect: () => onNavigate({ kind: 'docs' }) },
                { label: guide.title },
              ]
            : [{ label: 'Docs' }]
        }
        headingRef={heading}
        title={title}
        description={guide?.description ?? 'Help with your tasks, environments, and results.'}
      />
      {guide ? (
        <article className="task-docs__guide">
          {guide.scenarios.map((scenario) => (
            <section key={scenario.title}>
              <h2>{scenario.title}</h2>
              <dl>
                <dt>What you see</dt>
                <dd>{scenario.state}</dd>
                <dt>What you can do next</dt>
                <dd>{scenario.action}</dd>
                <dt>How to know it completed</dt>
                <dd>{scenario.completion}</dd>
              </dl>
            </section>
          ))}
        </article>
      ) : (
        <>
          <div className="task-docs__cards">
            {(Object.keys(TASK_GUIDES) as HelpTopic[]).map((topic) => (
              <DocsLink
                key={topic}
                view={topic}
                className="task-docs__card"
                onNavigate={onNavigate}
              >
                <h2>{TASK_GUIDES[topic].title}</h2>
                <p>{TASK_GUIDES[topic].description}</p>
                <span>Read guide →</span>
              </DocsLink>
            ))}
          </div>
          <section className="task-docs__integration" aria-labelledby="integration-help-title">
            <h2 id="integration-help-title">Build an integration</h2>
            <DocsLink
              view="event_gateway_protocol"
              className="task-docs__card"
              onNavigate={onNavigate}
            >
              <h3>Event Gateway Protocol</h3>
              <p>
                Integrate event sources, manage channels, implement providers, and deliver replies.
              </p>
              <span>Read documentation →</span>
            </DocsLink>
          </section>
        </>
      )}
    </section>
  );
}
