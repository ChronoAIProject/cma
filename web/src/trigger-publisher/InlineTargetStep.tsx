import { Picker } from '../ui/Picker';
import * as React from 'react';
import type { TriggerContent, TriggerInlineTarget } from '@cma/client';
import { Presence } from '../ui/Presence';
import { Input } from '../ui/Input';
import {
  imageVisibilityLabel,
  listAgentImages,
  resolveAgentImage,
  type AgentImageChoice,
} from '../create/imageCatalog';
import { inlineResultSentence } from './model';
import { useImageManagerAvailability } from '../services/ImageManagerRetirement';

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
  const imageManager = useImageManagerAvailability();
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
    if (imageManager !== 'legacy') return;
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
  }, [imageManager]);
  const updateTarget = (next: React.SetStateAction<TriggerInlineTarget>) =>
    onChange((current) => ({
      ...current,
      target: {
        ...current.target,
        inline: typeof next === 'function' ? next(current.target.inline!) : next,
      },
    }));
  // Merge into the latest target: an image resolution may have committed since
  // this render, and an edit must never restore the unresolved image.
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
      {imageManager !== 'retired' ? (
        <div className="trigger-field">
          <label htmlFor="trigger-inline-image">Image</label>
          <Picker
            id="trigger-inline-image"
            value={target.image?.id ?? ''}
            disabled={disabled || imageResolving || imageManager === 'loading'}
            onChange={(choice) => {
              const id = choice;
              const request = ++imageRequest.current;
              if (id === '') {
                setImageFailure(undefined);
                updateTarget((current) => ({ ...current, image: undefined }));
                return;
              }
              updateTarget((current) => ({ ...current, image: { id } }));
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
            label="Image"
            options={[
              { value: '', label: 'System default image' },
              ...images.map((image) => ({
                value: image.id,
                label: `${image.name} (${imageVisibilityLabel(image.visibility)})`,
              })),
            ]}
          />
          <Presence>
            <div>
              {imageResolving ? (
                <p className="trigger-caption">Resolving immutable image version…</p>
              ) : null}
              {imageFailure === undefined ? null : (
                <p className="trigger-caption">{imageFailure}</p>
              )}
              {errors['target.inline.image'] === undefined ? null : (
                <p className="trigger-error" role="alert">
                  {errors['target.inline.image']}
                </p>
              )}
            </div>
          </Presence>
        </div>
      ) : null}
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
