import { Picker } from '../ui/Picker';
import type { SetStateAction } from 'react';
import type { TriggerContent, TriggerInlineTarget } from '@cma/client';
import { Presence } from '../ui/Presence';
import { Input } from '../ui/Input';
import { inlineResultSentence } from './model';

export function InlineTargetStep({
  value,
  errors,
  disabled,
  onChange,
}: {
  readonly value: TriggerContent;
  readonly errors: Readonly<Record<string, string>>;
  readonly disabled: boolean;
  readonly onChange: (value: SetStateAction<TriggerContent>) => void;
}) {
  const target = value.target.inline as TriggerInlineTarget;
  const updateTarget = (next: SetStateAction<TriggerInlineTarget>) =>
    onChange((current) => ({
      ...current,
      target: {
        ...current.target,
        inline: typeof next === 'function' ? next(current.target.inline!) : next,
      },
    }));
  const updateAgent = (patch: Partial<TriggerInlineTarget['agent']>) =>
    updateTarget((current) => ({ ...current, agent: { ...current.agent, ...patch } }));
  const workspace = target.workspace ?? {};
  const repository = workspace.repository;
  const setRepository = (next: { url: string; ref?: string } | undefined) =>
    updateTarget((current) => ({
      ...current,
      workspace: next === undefined ? undefined : { repository: next },
    }));
  return (
    <>
      {errors['target.inline.image'] === undefined ? null : (
        <p className="trigger-error" role="alert">
          {errors['target.inline.image']}
        </p>
      )}
      <div className="trigger-field">
        <label htmlFor="trigger-inline-model">Model</label>
        <Input
          id="trigger-inline-model"
          value={target.agent.model ?? ''}
          disabled={disabled}
          placeholder="Deployment default"
          onChange={(event) => updateAgent({ model: event.target.value || undefined })}
        />
      </div>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-reasoning">Reasoning effort</label>
        <Picker
          id="trigger-inline-reasoning"
          value={target.agent.reasoning_effort}
          disabled={disabled}
          onChange={(choice) =>
            updateAgent({
              reasoning_effort: choice as typeof target.agent.reasoning_effort,
            })
          }
          label="Reasoning effort"
          options={(['minimal', 'low', 'medium', 'high', 'x_high'] as const).map((effort) => ({
            value: effort,
            label: effort,
          }))}
        />
      </div>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-permission">Permission level</label>
        <Picker
          id="trigger-inline-permission"
          value={target.agent.permission_mode}
          disabled={disabled}
          onChange={(choice) =>
            updateAgent({
              permission_mode: choice as typeof target.agent.permission_mode,
            })
          }
          label="Permission level"
          options={[
            { value: 'guarded', label: 'Guarded' },
            { value: 'autonomous', label: 'Autonomous' },
          ]}
        />
      </div>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-skills">Skills (comma separated)</label>
        <Input
          id="trigger-inline-skills"
          value={target.agent.skills.map((skill) => skill.id).join(', ')}
          disabled={disabled}
          onChange={(event) =>
            updateAgent({
              skills: event.target.value
                .split(',')
                .map((id) => id.trim())
                .filter(Boolean)
                .map((id) => ({ source: 'ornn' as const, id })),
            })
          }
        />
      </div>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-services">Agent key scope services (comma separated)</label>
        <Input
          id="trigger-inline-services"
          value={target.agent.services.join(', ')}
          disabled={disabled}
          onChange={(event) =>
            updateAgent({
              services: event.target.value
                .split(',')
                .map((service) => service.trim())
                .filter(Boolean),
            })
          }
        />
      </div>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-system-prompt">System prompt</label>
        <textarea
          id="trigger-inline-system-prompt"
          rows={4}
          value={target.agent.system_prompt}
          disabled={disabled}
          onChange={(event) => updateAgent({ system_prompt: event.target.value })}
        />
      </div>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-repository">GitHub repository (optional)</label>
        <Input
          id="trigger-inline-repository"
          value={repository?.url ?? ''}
          disabled={disabled}
          placeholder="https://github.com/org/repository"
          aria-invalid={errors['target.inline.workspace.repository'] !== undefined}
          onChange={(event) => {
            const url = event.target.value;
            setRepository(url === '' ? undefined : { url, ref: repository?.ref });
          }}
        />
        <Input
          aria-label="GitHub repository ref"
          value={repository?.ref ?? ''}
          disabled={disabled || repository === undefined}
          placeholder="main (optional)"
          onChange={(event) =>
            setRepository(
              repository === undefined
                ? undefined
                : { url: repository.url, ref: event.target.value || undefined },
            )
          }
        />
        <Presence>
          {errors['target.inline.workspace.repository'] === undefined ? null : (
            <p className="trigger-error" role="alert">
              {errors['target.inline.workspace.repository']}
            </p>
          )}
        </Presence>
      </div>
      <p className="trigger-result" role="status">
        {inlineResultSentence(target)}
      </p>
    </>
  );
}
