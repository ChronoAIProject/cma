import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState, type SetStateAction } from 'react';
import type { ProfileKind, TriggerContent, TriggerDocument } from '@cma/client';
import type { CommandOptions } from '../api/client';
import { Button } from '../ui/Button';
import { InlineConfirm } from '../ui/InlineConfirm';
import {
  IconChevronLeftOutline14,
  IconChevronRightOutline14,
  IconCheckOutline16,
  IconRefreshOutline16,
} from '../ui/icons';
import type { PublisherProfileChoice, TriggerPublisherClient } from './client';
import { PresentationStep, ReuseStep, TargetStep, TrustDisclosures } from './ContentSteps';
import { Placements } from './Placements';
import { InlineTargetReview } from './InlineTargetReview';
import {
  blankContent,
  inlineResultSentence,
  resultSentence,
  reuseExplanation,
  STEP_NAMES,
  validateStep,
  type SelectedProfiles,
} from './model';
import { usePublisherCommand } from './transaction';

interface CommandResult {
  readonly document: TriggerDocument;
  readonly published: boolean;
}

export function TriggerWizard({
  client,
  document,
  initialStep = 0,
  onChange,
  onPublished,
  onLock,
  onSaved,
}: {
  readonly client: TriggerPublisherClient;
  readonly document?: TriggerDocument;
  readonly initialStep?: number;
  readonly onChange: (document: TriggerDocument) => void;
  readonly onPublished: (document: TriggerDocument) => void;
  readonly onLock: (locked: boolean) => void;
  readonly onSaved: (document: TriggerDocument, step: number) => void;
}) {
  const [content, setContent] = useState<TriggerContent>(document?.draft ?? blankContent);
  const [step, setStep] = useState(initialStep);
  const [profiles, setProfiles] = useState<SelectedProfiles>({});
  const [reconciling, setReconciling] = useState(false);
  const [profilesKey, setProfilesKey] = useState('');
  const [profileErrors, setProfileErrors] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [confirmed, setConfirmed] = useState(false);
  const [placementLocked, setPlacementLocked] = useState(false);
  const [placementConflict, setPlacementConflict] = useState(false);
  const [reloadConfirm, setReloadConfirm] = useState(false);
  const [retiring, setRetiring] = useState(false);
  const [notice, setNotice] = useState('');
  const [profileReload, setProfileReload] = useState(0);
  const activeProfile = useRef<AbortController | undefined>(undefined);
  const heading = useRef<HTMLHeadingElement>(null);
  const titleInput = useRef<HTMLInputElement>(null);
  const command = usePublisherCommand<CommandResult>(
    (result) => {
      onChange(result.document);
      setContent(result.document.draft);
      setConfirmed(false);
      setRetiring(false);
      setNotice(
        result.published
          ? `Revision ${String(result.document.revision_count)} published`
          : result.document.availability === 'retired'
            ? 'Trigger retired'
            : `Draft version ${String(result.document.draft_version)} saved`,
      );
      if (result.published) onPublished(result.document);
      else onSaved(result.document, step);
    },
    (failure) => {
      if (failure.field?.startsWith('target.')) setStep(0);
      if (failure.field?.startsWith('presentation.') || failure.field === 'first_message')
        setStep(2);
    },
  );
  useEffect(() => {
    onLock(command.locked || placementLocked);
    return () => onLock(false);
  }, [command.locked, placementLocked, onLock]);
  const targetsKey = JSON.stringify(content.target);
  const profileRequestKey = `${targetsKey}:${String(profileReload)}`;
  const profileBusy =
    reconciling || (content.target.inline === undefined && profilesKey !== profileRequestKey);
  useEffect(() => {
    const request = new AbortController();
    activeProfile.current?.abort();
    activeProfile.current = request;
    const target = JSON.parse(targetsKey) as TriggerContent['target'];
    if (target.inline !== undefined) {
      queueMicrotask(() => {
        if (request.signal.aborted) return;
        setProfiles({});
        setProfileErrors({});
        setProfilesKey(profileRequestKey);
      });
      return () => request.abort();
    }
    void Promise.all(
      (['sandbox', 'workspace', 'agent'] as const).map(async (kind) => {
        const reference = target[`${kind}_profile`];
        if (reference.id === '') return { kind };
        try {
          return {
            kind,
            profile: await client.readProfile(kind, reference, { signal: request.signal }),
          };
        } catch {
          return {
            kind,
            error: 'This pinned profile revision could not be loaded. Choose it again or retry.',
          };
        }
      }),
    ).then((results) => {
      if (request.signal.aborted) return;
      const next: SelectedProfiles = {};
      const failures: Record<string, string> = {};
      for (const result of results) {
        if ('profile' in result && result.profile !== undefined) next[result.kind] = result.profile;
        if ('error' in result && result.error !== undefined)
          failures[`target.${result.kind}_profile`] = result.error;
      }
      setProfiles(next);
      setProfileErrors(failures);
      setProfilesKey(profileRequestKey);
    });
    return () => request.abort();
  }, [client, targetsKey, profileRequestKey]);
  useEffect(() => {
    heading.current?.focus();
  }, [step]);
  const titleValidation =
    errors['presentation.title'] !== undefined
      ? errors
      : command.failure?.field === 'presentation.title'
        ? command.failure
        : undefined;
  useEffect(() => {
    if (step === 2 && titleValidation !== undefined) titleInput.current?.focus();
  }, [step, titleValidation]);
  const update = (next: SetStateAction<TriggerContent>) => {
    setContent(next);
    setConfirmed(false);
    setNotice('');
    setErrors({});
  };
  const select = (kind: ProfileKind, choice: PublisherProfileChoice) =>
    update({
      ...content,
      target: {
        ...content.target,
        [`${kind}_profile`]: { id: choice.id, revision: choice.revision },
      },
    });
  const allErrors = {
    ...validateStep(0, content, profiles),
    ...validateStep(2, content, profiles),
  };
  const locked = command.locked || placementLocked || profileBusy;
  const unavailable = document?.availability === 'retired';
  const saveDraft = async (options: CommandOptions): Promise<TriggerDocument> => {
    const body = structuredClone(content);
    if (document === undefined) {
      const created = await client.triggers.create({ content: body }, options);
      if (!options.signal?.aborted) onChange(created);
      return created;
    }
    if (JSON.stringify(document.draft) === JSON.stringify(body)) return document;
    const saved = await client.triggers.save(
      document.id,
      { content: body, draft_version: document.draft_version },
      options,
    );
    if (!options.signal?.aborted) onChange(saved);
    return saved;
  };
  const save = (publish: boolean) => {
    setErrors(allErrors);
    if (Object.keys(allErrors).length > 0) {
      setStep(Object.keys(validateStep(0, content, profiles)).length > 0 ? 0 : 2);
      return;
    }
    if (publish && !confirmed) return;
    let saved: TriggerDocument | undefined;
    void command.submit(async (options) => {
      saved ??= await saveDraft({ ...options, idempotencyKey: `${options.idempotencyKey}:draft` });
      if (!publish) return { document: saved, published: false };
      const revision = await client.triggers.publish(
        saved.id,
        { draft_version: saved.draft_version },
        { ...options, idempotencyKey: `${options.idempotencyKey}:publish` },
      );
      return {
        document: {
          ...saved,
          draft: revision.content,
          public_id: revision.public_id,
          availability: revision.availability,
          revision_count: revision.revision,
        },
        published: true,
      };
    });
  };
  const reload = async () => {
    if (document === undefined || locked) return;
    setReconciling(true);
    try {
      const current = await client.triggers.read(document.id);
      onChange(current);
      setContent(current.draft);
      setErrors({});
      setConfirmed(false);
      setReloadConfirm(false);
      setPlacementConflict(false);
      setNotice('');
      command.clearFailure();
      setProfileReload((value) => value + 1);
    } catch {
      setNotice('The current draft could not be loaded. Your changes are still here.');
    } finally {
      setReconciling(false);
    }
  };
  const failureField = command.failure?.field;
  const normalizedField =
    (['sandbox', 'workspace', 'agent'] as const)
      .map((kind) => `target.${kind}_profile`)
      .find((field) => failureField?.startsWith(field)) ?? failureField;
  const fieldErrors = {
    ...errors,
    ...profileErrors,
    ...(normalizedField === undefined ? {} : { [normalizedField]: command.failure?.message ?? '' }),
  };
  const revision = (document?.revision_count ?? 0) + 1;
  const conflict = command.failure?.conflict === true || placementConflict;
  return (
    <section className="trigger-wizard" aria-labelledby="trigger-step-heading">
      <ol className="trigger-steps" aria-label="Publishing steps">
        {STEP_NAMES.map((name, index) => (
          <li key={name} aria-current={step === index ? 'step' : undefined}>
            <span>{index + 1}</span>
            {name}
          </li>
        ))}
      </ol>
      <h2 id="trigger-step-heading" ref={heading} tabIndex={-1}>
        {STEP_NAMES[step]}
      </h2>
      {unavailable ? (
        <p className="trigger-result">
          This Trigger is retired. Existing launch records remain available.
        </p>
      ) : null}
      {step === 0 ? (
        <>
          <TargetStep
            client={client}
            value={content}
            profiles={profiles}
            errors={fieldErrors}
            disabled={locked || unavailable}
            onChange={update}
            onSelect={select}
          />
          {Object.keys(profileErrors).length === 0 ? null : (
            <Button
              icon={<IconRefreshOutline16 />}
              disabled={locked}
              onClick={() => setProfileReload((value) => value + 1)}
            >
              Retry profile revisions
            </Button>
          )}
        </>
      ) : null}
      {step === 1 ? (
        <ReuseStep
          value={content.reuse}
          disabled={locked || unavailable}
          onChange={(reuse) => update({ ...content, reuse })}
        />
      ) : null}
      {step === 2 ? (
        <PresentationStep
          value={content}
          errors={fieldErrors}
          titleInput={titleInput}
          disabled={locked || unavailable}
          onChange={update}
        />
      ) : null}
      {step === 3 ? (
        <Placements
          key={profileReload}
          client={client}
          document={document}
          disabled={
            command.locked ||
            conflict ||
            profileBusy ||
            unavailable ||
            Object.keys(allErrors).length > 0
          }
          saveDraft={saveDraft}
          onChange={onChange}
          onLock={setPlacementLocked}
          onConflict={() => setPlacementConflict(true)}
        />
      ) : null}
      {step === 4 ? (
        <div className="trigger-review">
          <h3>{content.presentation.title || 'Untitled Trigger'}</h3>
          <p>
            {content.target.inline
              ? inlineResultSentence(content.target.inline)
              : resultSentence(profiles)}
          </p>
          <p>{reuseExplanation(content.reuse)}</p>
          {content.target.inline ? (
            <InlineTargetReview target={content.target.inline} />
          ) : (
            <dl>
              {(['sandbox', 'workspace', 'agent'] as const).map((kind) => (
                <div key={kind}>
                  <dt>{kind} profile</dt>
                  <dd>
                    <ReferenceLink
                      kind={kind}
                      id={content.target[`${kind}_profile`].id}
                      name={profiles[kind]?.name}
                    />
                    , revision {content.target[`${kind}_profile`].revision}
                  </dd>
                </div>
              ))}
            </dl>
          )}
          {content.first_message === '' ? null : (
            <div className="trigger-first-message">
              <h3>First message</h3>
              <pre>{content.first_message}</pre>
            </div>
          )}
          <TrustDisclosures profiles={profiles} />
          <label className="trigger-checkbox">
            <input
              type="checkbox"
              checked={confirmed}
              disabled={locked || unavailable}
              onChange={(event) => setConfirmed(event.target.checked)}
            />
            Publish revision {revision} of {content.presentation.title || 'this Trigger'}
          </label>
        </div>
      ) : null}
      <Presence>
        {command.failure === undefined && !placementConflict ? null : (
          <div className="trigger-failure" role="alert">
            <p>
              {command.failure?.message ?? 'The saved draft changed. Your changes are still here.'}
            </p>
            {command.failure?.uncertain ? (
              <Button disabled={command.pending} onClick={command.retry}>
                Retry original request
              </Button>
            ) : null}
            {conflict ? (
              reloadConfirm ? (
                <InlineConfirm
                  question="Replace your unsaved changes with the current saved draft?"
                  confirmLabel="Load current draft"
                  busy={locked}
                  onConfirm={() => void reload()}
                  onCancel={() => setReloadConfirm(false)}
                />
              ) : (
                <Button disabled={locked} onClick={() => setReloadConfirm(true)}>
                  Load current draft
                </Button>
              )
            ) : null}
          </div>
        )}
      </Presence>
      <Presence>
        {notice || command.pending || profileBusy ? (
          <p className="trigger-notice" role="status">
            {notice ||
              (command.pending ? 'Saving Trigger' : profileBusy ? 'Loading profile revisions' : '')}
          </p>
        ) : null}
      </Presence>
      <footer className="trigger-actions">
        <Button
          icon={<IconChevronLeftOutline14 />}
          disabled={locked || step === 0}
          onClick={() => {
            setStep((value) => value - 1);
            setErrors({});
          }}
        >
          Back
        </Button>
        <Button disabled={locked || unavailable || conflict} onClick={() => save(false)}>
          Save draft
        </Button>
        {step < 4 ? (
          <Button
            variant="primary"
            icon={<IconChevronRightOutline14 />}
            disabled={locked || unavailable}
            onClick={() => {
              const validation = validateStep(step, content, profiles);
              setErrors(validation);
              if (Object.keys(validation).length === 0) setStep((value) => value + 1);
            }}
          >
            Next
          </Button>
        ) : (
          <Button
            variant="primary"
            icon={<IconCheckOutline16 />}
            disabled={locked || unavailable || !confirmed || conflict}
            onClick={() => save(true)}
          >
            Publish revision {revision}
          </Button>
        )}
      </footer>
      {document?.availability === 'published' ? (
        <div className="trigger-retire">
          {retiring ? (
            <InlineConfirm
              question={`Retire ${document.draft.presentation.title}? New launches will be refused; existing launches remain.`}
              confirmLabel="Retire Trigger"
              busy={locked}
              cancelDisabled={command.locked}
              onCancel={() => setRetiring(false)}
              onConfirm={() => {
                void command.submit(async (options) => ({
                  document: await client.triggers.retire(document.id, options),
                  published: false,
                }));
              }}
            />
          ) : (
            <Button disabled={locked} onClick={() => setRetiring(true)}>
              Retire Trigger
            </Button>
          )}
        </div>
      ) : null}
    </section>
  );
}
import { ReferenceLink } from '../profiles/ReferenceLink';
