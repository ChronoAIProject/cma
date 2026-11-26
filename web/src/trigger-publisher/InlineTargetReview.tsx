import type { TriggerInlineTarget } from '@cma/client';

export function InlineTargetReview({ target }: { readonly target: TriggerInlineTarget }) {
  const repository = target.workspace?.repository;
  return (
    <>
      <dl>
        <div>
          <dt>Image</dt>
          <dd>{target.image?.id ?? 'System default image'}</dd>
        </div>
        {target.image?.version ? (
          <div>
            <dt>Image version</dt>
            <dd>{target.image.version}</dd>
          </div>
        ) : null}
        <div>
          <dt>Workspace</dt>
          <dd>
            {repository === undefined
              ? 'Empty workspace'
              : `${repository.url}${repository.ref ? ` · ${repository.ref}` : ''}`}
          </dd>
        </div>
        <div>
          <dt>Model</dt>
          <dd>{target.agent.model ?? 'Deployment default'}</dd>
        </div>
        <div>
          <dt>Reasoning effort</dt>
          <dd>{target.agent.reasoning_effort}</dd>
        </div>
        <div>
          <dt>Permission level</dt>
          <dd>{target.agent.permission_mode}</dd>
        </div>
        <div>
          <dt>Agent key scope</dt>
          <dd>{target.agent.services.join(', ') || 'None selected'}</dd>
        </div>
        <div>
          <dt>Skills</dt>
          <dd>
            {target.agent.skills
              .map((skill) => `${skill.id}${skill.version ? ` · ${skill.version}` : ''}`)
              .join(', ') || 'None selected'}
          </dd>
        </div>
      </dl>
      {target.agent.system_prompt ? (
        <div className="trigger-first-message">
          <h3>System prompt</h3>
          <pre>{target.agent.system_prompt}</pre>
        </div>
      ) : null}
    </>
  );
}
