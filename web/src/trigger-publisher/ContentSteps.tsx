import { Picker } from '../ui/Picker';
import { Presence } from '../ui/Presence';
import type { Ref, SetStateAction } from 'react';
import type { ProfileKind, TriggerContent } from '@cma/client';
import { Input } from '../ui/Input';
import { ChoiceGroup } from '../profiles/ChoiceGroup';
import type { PublisherProfileChoice, TriggerPublisherClient } from './client';
import { DraftBadge } from './BadgePreview';
import { LABELS, resultSentence, reuseExplanation, type SelectedProfiles } from './model';
import { ProfilePicker } from './ProfilePicker';
import { QuickMessageFields } from './QuickMessageFields';
import { InlineTargetStep } from './InlineTargetStep';
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
  readonly onChange: (value: SetStateAction<TriggerContent>) => void;
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
  titleInput,
  onChange,
}: {
  readonly value: TriggerContent;
  readonly disabled: boolean;
  readonly errors: Readonly<Record<string, string>>;
  readonly titleInput: Ref<HTMLInputElement>;
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
        const fieldId = `trigger-${name}`;
        const errorId = `${fieldId}-error`;
        const text = name === 'first_message' ? value.first_message : (presentation[name] ?? '');
        const change = (text: string) =>
          name === 'first_message'
            ? onChange({ ...value, first_message: text })
            : field(name, text);
        return (
          <div key={name} className="trigger-field">
            <label htmlFor={fieldId}>{label}</label>
            {name === 'title' || name === 'signup_hint' ? (
              <Input
                id={fieldId}
                ref={name === 'title' ? titleInput : undefined}
                value={text}
                disabled={disabled}
                aria-invalid={error !== undefined}
                aria-describedby={error === undefined ? undefined : errorId}
                onChange={(event) => change(event.target.value)}
              />
            ) : (
              <textarea
                id={fieldId}
                rows={name === 'first_message' ? 5 : 3}
                value={text}
                disabled={disabled}
                aria-invalid={error !== undefined}
                aria-describedby={error === undefined ? undefined : errorId}
                onChange={(event) => change(event.target.value)}
              />
            )}
            <Presence>
              {error === undefined ? null : (
                <p id={errorId} className="trigger-error" role="alert">
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
        <Picker
          id="trigger-theme"
          value={presentation.theme}
          disabled={disabled}
          onChange={(choice) =>
            onChange({
              ...value,
              presentation: {
                ...presentation,
                theme: choice as typeof presentation.theme,
              },
            })
          }
          label="Theme"
          options={[
            { value: 'auto', label: 'Match the visitor' },
            { value: 'light', label: 'Light' },
            { value: 'dark', label: 'Dark' },
          ]}
        />
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
