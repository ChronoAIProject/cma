import * as React from 'react';
import type { TriggerContent, TriggerInlineTarget } from '@chronoai/cma-sdk';
import { Presence } from '../ui/Presence';
import { Input } from '../ui/Input';
import {
  imageVisibilityLabel,
  listAgentImages,
  resolveAgentImage,
  type AgentImageChoice,
} from '../create/imageCatalog';
import { inlineResultSentence } from './model';

export function InlineTargetStep({
  value,
  errors,
  disabled,
  onChange,
  onImageResolving,
}: {
  readonly value: TriggerContent;
  readonly errors: Readonly<Record<string, string>>;
  readonly disabled: boolean;
  readonly onChange: (value: React.SetStateAction<TriggerContent>) => void;
  readonly onImageResolving: (resolving: boolean) => void;
}) {
  const target = value.target.inline as TriggerInlineTarget;
  const [images, setImages] = React.useState<readonly AgentImageChoice[]>([]);
  const [imageFailure, setImageFailure] = React.useState<string>();
  const [imageResolving, setImageResolving] = React.useState(false);
  React.useEffect(() => {
    onImageResolving(imageResolving);
    return () => onImageResolving(false);
  }, [imageResolving, onImageResolving]);
  const imageRequest = React.useRef(0);
  const imageController = React.useRef<AbortController | undefined>(undefined);
  React.useEffect(() => {
    const controller = new AbortController();
    void listAgentImages(controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) {
          setImages(next.filter((image) => image.retired !== true));
          setImageFailure(undefined);
        }
      })
      .catch(() => {
        if (!controller.signal.aborted)
          setImageFailure(
            'Image Manager is unavailable. Use the system default image or try again later.',
          );
      });
    return () => {
      controller.abort();
      imageController.current?.abort();
    };
  }, []);
  const updateTarget = (next: React.SetStateAction<TriggerInlineTarget>) =>
    onChange((current) => ({
      ...current,
      target: {
        ...current.target,
        inline: typeof next === 'function' ? next(current.target.inline!) : next,
      },
    }));
  const updateAgent = (patch: Partial<TriggerInlineTarget['agent']>) =>
    updateTarget({ ...target, agent: { ...target.agent, ...patch } });
  const workspace = target.workspace ?? {};
  const repository = workspace.repository;
  const setRepository = (next: { url: string; reference?: string } | undefined) =>
    updateTarget({
      ...target,
      workspace: next === undefined ? undefined : { repository: next },
    });
  return (
    <>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-image">Image</label>
        <select
          id="trigger-inline-image"
          value={target.image?.id ?? ''}
          disabled={disabled || imageResolving}
          onChange={(event) => {
            const id = event.target.value;
            const request = ++imageRequest.current;
            if (id === '') {
              setImageFailure(undefined);
              updateTarget({ ...target, image: undefined });
              return;
            }
            updateTarget({ ...target, image: { id } });
            setImageResolving(true);
            setImageFailure(undefined);
            imageController.current?.abort();
            const controller = new AbortController();
            imageController.current = controller;
            void resolveAgentImage(id, controller.signal)
              .then((resolved) => {
                if (request !== imageRequest.current || controller.signal.aborted) return;
                updateTarget((current) => ({ ...current, image: resolved }));
              })
              .catch(() => {
                if (request !== imageRequest.current || controller.signal.aborted) return;
                setImageFailure('The selected image has no successful immutable version.');
              })
              .finally(() => {
                if (request === imageRequest.current) setImageResolving(false);
              });
          }}
        >
          <option value="">System default image</option>
          {images.map((image) => (
            <option key={image.id} value={image.id}>
              {image.name} ({imageVisibilityLabel(image.visibility)})
            </option>
          ))}
        </select>
        <Presence>
          <div>
            {imageResolving ? (
              <p className="trigger-caption">Resolving immutable image version…</p>
            ) : null}
            {imageFailure === undefined ? null : <p className="trigger-caption">{imageFailure}</p>}
            {errors['target.inline.image'] === undefined ? null : (
              <p className="trigger-error" role="alert">
                {errors['target.inline.image']}
              </p>
            )}
          </div>
        </Presence>
      </div>
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
        <select
          id="trigger-inline-reasoning"
          value={target.agent.reasoning_effort}
          disabled={disabled}
          onChange={(event) =>
            updateAgent({
              reasoning_effort: event.target.value as typeof target.agent.reasoning_effort,
            })
          }
        >
          {(['minimal', 'low', 'medium', 'high', 'x_high'] as const).map((effort) => (
            <option key={effort} value={effort}>
              {effort}
            </option>
          ))}
        </select>
      </div>
      <div className="trigger-field">
        <label htmlFor="trigger-inline-permission">Permission level</label>
        <select
          id="trigger-inline-permission"
          value={target.agent.permission_mode}
          disabled={disabled}
          onChange={(event) =>
            updateAgent({
              permission_mode: event.target.value as typeof target.agent.permission_mode,
            })
          }
        >
          <option value="guarded">Guarded</option>
          <option value="autonomous">Autonomous</option>
        </select>
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
            setRepository(url === '' ? undefined : { url, reference: repository?.reference });
          }}
        />
        <Input
          aria-label="GitHub repository ref"
          value={repository?.reference ?? ''}
          disabled={disabled || repository === undefined}
          placeholder="main (optional)"
          onChange={(event) =>
            setRepository(
              repository === undefined
                ? undefined
                : { url: repository.url, reference: event.target.value || undefined },
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
