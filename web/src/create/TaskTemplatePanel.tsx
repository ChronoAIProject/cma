import type { ReactNode } from 'react';
import { TaskTemplateFields, type TaskTemplateAvailability } from './TaskTemplateFields';
import type { TaskTemplate, TemplateIssue } from './taskTemplateModel';

export function TaskTemplatePanel({
  template,
  issue,
  errorId,
  availability,
  allowV2 = false,
  onChange,
  children,
}: {
  readonly template?: TaskTemplate;
  readonly issue?: TemplateIssue;
  readonly errorId: string;
  readonly availability: TaskTemplateAvailability;
  readonly allowV2?: boolean;
  readonly onChange: (template: TaskTemplate) => void;
  readonly children: ReactNode;
}) {
  if (!template) return children;
  return (
    <div className="task-template-panel">
      {availability !== 'supported' ? (
        <div className="task-template-panel__notice" role="status">
          <strong>
            {availability === 'checking'
              ? 'Checking template availability…'
              : availability === 'error'
                ? 'Template availability could not be checked.'
                : 'Template tasks are not available on this service.'}
          </strong>
          <p>Your fields are kept. You can keep editing or write a task without a template.</p>
        </div>
      ) : null}
      <TaskTemplateFields
        template={template}
        issue={issue}
        errorId={errorId}
        onChange={onChange}
        allowV2={allowV2}
      />
      {children}
    </div>
  );
}
