import { Presence } from '../ui/Presence';
import * as React from 'react';
import type { ProfileKind, TriggerContent, TriggerInlineTarget } from '@chronoai/cma-sdk';
import { Input } from '../ui/Input';
import { ChoiceGroup } from '../profiles/ChoiceGroup';
import type { PublisherProfileChoice, TriggerPublisherClient } from './client';
import { DraftBadge } from './BadgePreview';
import {
  inlineResultSentence,
  LABELS,
  resultSentence,
  reuseExplanation,
  type SelectedProfiles,
} from './model';
import { ProfilePicker } from './ProfilePicker';
import { QuickMessageFields } from './QuickMessageFields';
import {
  imageVisibilityLabel,
  listAgentImages,
  resolveAgentImage,
  type AgentImageChoice,
} from '../create/imageCatalog';
import { TrustDisclosures } from './TrustDisclosures';

export { TrustDisclosures } from './TrustDisclosures';

export function TargetStep({
  client,
  value,
  profiles,
  errors,
  disabled,
  onChange,
  onSelect,
}: {
  readonly client: TriggerPublisherClient;
  readonly value: TriggerContent;
  readonly profiles: SelectedProfiles;
  readonly errors: Readonly<Record<string, string>>;
  readonly disabled: boolean;
  readonly onChange: (value: TriggerContent) => void;
  readonly onSelect: (kind: ProfileKind, choice: PublisherProfileChoice) => void;
}) {
  if (value.target.inline !== undefined) {
    return (
      <InlineTargetStep value={value} errors={errors} disabled={disabled} onChange={onChange} />
    );
  }
  const sandbox = profiles.sandbox;
  const sentence = resultSentence(profiles);
  return (
    <>
      {(['sandbox', 'workspace', 'agent'] as const).map((kind) => (
        <ProfilePicker
          key={kind}
          client={client}
          kind={kind}
          value={profiles[kind]}
          references={
            kind === 'workspace' && sandbox?.kind === 'sandbox'
              ? sandbox.content.workspace_profiles
              : undefined
          }
          disabled={disabled}
          error={errors[`target.${kind}_profile`]}
          onSelect={(choice) => onSelect(kind, choice)}
        />
      ))}
      <Presence>
        {sentence === '' ? null : (
          <p className="trigger-result" role="status">
            {sentence}
          </p>
        )}
      </Presence>
      <TrustDisclosures profiles={profiles} />
    </>
  );
}

function InlineTargetStep({
  value,
  errors,
  disabled,
  onChange,
}: {
  readonly value: TriggerContent;
  readonly errors: Readonly<Record<string, string>>;
  readonly disabled: boolean;
  readonly onChange: (value: TriggerContent) => void;
}) {
  const target = value.target.inline as TriggerInlineTarget;
  const [images, setImages] = React.useState<readonly AgentImageChoice[]>([]);
  const [imageFailure, setImageFailure] = React.useState<string>();
  const [imageResolving, setImageResolving] = React.useState(false);
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
  const updateTarget = (next: TriggerInlineTarget) =>
    onChange({ ...value, target: { ...value.target, inline: next } });
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
            setImageResolving(true);
            setImageFailure(undefined);
            imageController.current?.abort();
            const controller = new AbortController();
            imageController.current = controller;
            void resolveAgentImage(id, controller.signal)
              .then((resolved) => {
                if (request !== imageRequest.current || controller.signal.aborted) return;
                updateTarget({ ...target, image: resolved });
              })
              .catch(() => {
                if (request !== imageRequest.current || controller.signal.aborted) return;
                setImageFailure('The selected image has no successful immutable version.');
                updateTarget({ ...target, image: { id } });
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

export function ReuseStep({
  value,
  disabled,
  onChange,
}: {
  readonly value: TriggerContent['reuse'];
  readonly disabled: boolean;
  readonly onChange: (value: TriggerContent['reuse']) => void;
}) {
  return (
    <>
      {(['sandbox', 'workspace'] as const).map((kind) => (
        <div className="trigger-field" key={kind}>
          <span className="trigger-field__label">{kind}</span>
          <ChoiceGroup
            label={`${kind} reuse`}
            value={value[kind]}
            disabled={disabled}
            options={[
              { value: 'reuse_existing', label: 'Use their existing one' },
              { value: 'always_new', label: 'Always create a new one' },
            ]}
            onChange={(next) => onChange({ ...value, [kind]: next })}
          />
        </div>
      ))}
      <div className="trigger-field">
        <span className="trigger-field__label">Agent</span>
        <ChoiceGroup
          label="Agent reuse"
          value={value.agent}
          disabled={disabled}
          options={[
            { value: 'resume_existing', label: 'Continue the previous agent' },
            { value: 'always_new', label: 'Always start a new one' },
            { value: 'ask', label: 'Ask them' },
          ]}
          onChange={(agent) => onChange({ ...value, agent })}
        />
      </div>
      <p className="trigger-result" role="status">
        {reuseExplanation(value)}
      </p>
    </>
  );
}

export function PresentationStep({
  value,
  disabled,
  errors,
  onChange,
}: {
  readonly value: TriggerContent;
  readonly disabled: boolean;
  readonly errors: Readonly<Record<string, string>>;
  readonly onChange: (value: TriggerContent) => void;
}) {
  const presentation = value.presentation;
  const field = (name: 'title' | 'description' | 'signup_hint', text: string) =>
    onChange({ ...value, presentation: { ...presentation, [name]: text } });
  return (
    <>
      {(['title', 'description', 'first_message', 'signup_hint'] as const).map((name) => {
        const label =
          name === 'first_message'
            ? 'First message'
            : name === 'signup_hint'
              ? 'Sign-up hint (optional)'
              : name;
        const error = errors[name === 'first_message' ? name : `presentation.${name}`];
        const text = name === 'first_message' ? value.first_message : (presentation[name] ?? '');
        const change = (text: string) =>
          name === 'first_message'
            ? onChange({ ...value, first_message: text })
            : field(name, text);
        return (
          <div key={name} className="trigger-field">
            <label htmlFor={`trigger-${name}`}>{label}</label>
            {name === 'title' || name === 'signup_hint' ? (
              <Input
                id={`trigger-${name}`}
                value={text}
                disabled={disabled}
                aria-invalid={error !== undefined}
                onChange={(event) => change(event.target.value)}
              />
            ) : (
              <textarea
                id={`trigger-${name}`}
                rows={name === 'first_message' ? 5 : 3}
                value={text}
                disabled={disabled}
                aria-invalid={error !== undefined}
                onChange={(event) => change(event.target.value)}
              />
            )}
            <Presence>
              {error === undefined ? null : (
                <p className="trigger-error" role="alert">
                  {error}
                </p>
              )}
            </Presence>
          </div>
        );
      })}
      <QuickMessageFields
        value={value.quick_messages ?? []}
        disabled={disabled}
        error={errors.quick_messages}
        onChange={(quick_messages) => onChange({ ...value, quick_messages })}
      />
      <fieldset className="trigger-badge-options" disabled={disabled}>
        <legend>Button label</legend>
        {(Object.keys(LABELS) as (keyof typeof LABELS)[]).map((label) => (
          <label key={label}>
            <input
              type="radio"
              name="trigger-button-label"
              value={label}
              checked={presentation.button_label === label}
              onChange={() =>
                onChange({ ...value, presentation: { ...presentation, button_label: label } })
              }
            />
            <DraftBadge label={label} theme={presentation.theme} />
          </label>
        ))}
      </fieldset>
      <div className="trigger-field">
        <label htmlFor="trigger-theme">Theme</label>
        <select
          id="trigger-theme"
          value={presentation.theme}
          disabled={disabled}
          onChange={(event) =>
            onChange({
              ...value,
              presentation: {
                ...presentation,
                theme: event.target.value as typeof presentation.theme,
              },
            })
          }
        >
          <option value="auto">Match the visitor</option>
          <option value="light">Light</option>
          <option value="dark">Dark</option>
        </select>
      </div>
      <label className="trigger-checkbox">
        <input
          type="checkbox"
          checked={value.consent.always_review}
          disabled={disabled}
          onChange={(event) =>
            onChange({ ...value, consent: { always_review: event.target.checked } })
          }
        />
        Show a review page first
      </label>
    </>
  );
}
