import { useEffect, useRef } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import type {
  AgentProfileContent,
  ProfileDocument,
  ProfileDraft,
  ProfileEditorClient,
  SkillCatalogClient,
} from './client';
import { useProfileEditor } from './editor';
import { validateAgentProfile } from './validation';
import { Field, fieldDescription } from './Field';
import { ChoiceGroup } from './ChoiceGroup';
import { EditorActions } from './EditorActions';
import { SkillPicker } from './pickers/SkillPicker';
import './Profiles.css';

const INITIAL: ProfileDraft<AgentProfileContent> = {
  name: '',
  description: '',
  content: { thinkingEffort: 'medium', permissionLevel: 'guarded', skills: [], systemPrompt: '' },
};

export interface AgentProfileEditorProps {
  readonly client: ProfileEditorClient<AgentProfileContent>;
  readonly catalog?: SkillCatalogClient;
  readonly profileId?: string;
  readonly onSaved?: (profile: ProfileDocument<AgentProfileContent>) => void;
}

export function AgentProfileEditor(props: AgentProfileEditorProps) {
  return <AgentEditor key={props.profileId ?? 'new'} {...props} />;
}

function AgentEditor({ client, catalog, profileId, onSaved }: AgentProfileEditorProps) {
  const editor = useProfileEditor(client, profileId, INITIAL, validateAgentProfile, onSaved);
  const form = useRef<HTMLFormElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const { draft, errors, locked } = editor;
  const title = editor.profile?.name ?? 'New agent profile';
  useEffect(() => {
    document.title = `${title} · Profiles · CMA`;
    return () => {
      document.title = 'CMA';
    };
  }, [title]);
  useEffect(() => {
    if (!editor.loading) heading.current?.focus();
  }, [editor.loading]);
  useEffect(() => {
    if (editor.validationAttempt > 0)
      form.current?.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();
  }, [editor.validationAttempt]);
  const content = (next: Partial<AgentProfileContent>) =>
    editor.change({ ...draft, content: { ...draft.content, ...next } });
  const propsFor = (field: string) => ({
    id: `profile-${field}`,
    'aria-invalid': errors[field] === undefined ? undefined : (true as const),
    'aria-describedby': fieldDescription(`profile-${field}`, errors[field]),
    disabled: locked,
  });
  if (editor.loading)
    return (
      <section className="profile-editor" aria-busy="true">
        <h1>Agent profile</h1>
        <p role="status">Loading profile</p>
      </section>
    );
  if (editor.readError !== undefined)
    return (
      <section className="profile-editor" role="alert">
        <h1 ref={heading} tabIndex={-1}>
          Profile unavailable
        </h1>
        <p>{editor.readError}</p>
        <Button onClick={editor.reload}>Try again</Button>
      </section>
    );
  return (
    <section className="profile-editor">
      <header className="profile-editor__header">
        <div>
          <p className="profile-editor__eyebrow">Agent profile</p>
          <h1 ref={heading} tabIndex={-1}>
            {title}
          </h1>
        </div>
        {editor.profile?.retiredAt === undefined ? null : <span>Retired</span>}
      </header>
      <form
        ref={form}
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          editor.save();
        }}
      >
        <section className="profile-editor__band">
          <h2>Profile</h2>
          <Field
            id="profile-name"
            label="Name"
            help="Choose a name that identifies this agent setup."
            error={errors.name}
          >
            <Input
              {...propsFor('name')}
              value={draft.name}
              onChange={(event) => editor.change({ ...draft, name: event.target.value })}
            />
          </Field>
          <Field
            id="profile-description"
            label="Description"
            help="Describe when someone should use this profile."
            error={errors.description}
          >
            <textarea
              {...propsFor('description')}
              className="profile-textarea"
              rows={3}
              value={draft.description}
              onChange={(event) => editor.change({ ...draft, description: event.target.value })}
            />
          </Field>
        </section>
        <section className="profile-editor__band">
          <h2>Agent</h2>
          <Field
            id="profile-model"
            label="Model"
            help="Leave this empty to use the deployment default."
            error={errors.model}
          >
            <Input
              {...propsFor('model')}
              value={draft.content.model ?? ''}
              placeholder="Deployment default"
              onChange={(event) =>
                content({ model: event.target.value === '' ? undefined : event.target.value })
              }
            />
          </Field>
          <div className="profile-field">
            <span className="profile-field__label">Thinking effort</span>
            <ChoiceGroup
              label="Thinking effort"
              value={draft.content.thinkingEffort}
              options={[
                { value: 'minimal', label: 'Minimal' },
                { value: 'low', label: 'Low' },
                { value: 'medium', label: 'Medium' },
                { value: 'high', label: 'High' },
                { value: 'x_high', label: 'Very high' },
              ]}
              disabled={locked}
              describedBy="profile-thinking-help"
              onChange={(thinkingEffort) => content({ thinkingEffort })}
            />
            <p id="profile-thinking-help" className="profile-field__help">
              Higher effort gives the agent more time to reason through a task.
            </p>
            {errors.thinkingEffort === undefined ? null : (
              <p className="profile-field__error">{errors.thinkingEffort}</p>
            )}
          </div>
          <div className="profile-field">
            <span className="profile-field__label">How freely it acts</span>
            <ChoiceGroup
              label="How freely it acts"
              value={draft.content.permissionLevel}
              options={[
                { value: 'guarded', label: 'Guarded' },
                { value: 'autonomous', label: 'Autonomous' },
              ]}
              disabled={locked}
              describedBy="profile-permission-help"
              onChange={(permissionLevel) => content({ permissionLevel })}
            />
            <p id="profile-permission-help" className="profile-field__help">
              Guarded asks before sensitive actions; Autonomous allows the agent to act without
              routine approval.
            </p>
            {errors.permissionLevel === undefined ? null : (
              <p className="profile-field__error">{errors.permissionLevel}</p>
            )}
          </div>
        </section>
        <section className="profile-editor__band">
          <h2>Skills</h2>
          <SkillPicker
            catalog={catalog}
            value={draft.content.skills}
            disabled={locked}
            error={errors.skills}
            onChange={(skills) => content({ skills })}
          />
          {errors.skills === undefined ? null : (
            <p id="profile-skills-error" className="profile-field__error">
              {errors.skills}
            </p>
          )}
        </section>
        <section className="profile-editor__band">
          <h2>Instructions</h2>
          <Field
            id="profile-systemPrompt"
            label="System prompt"
            help="These plain-text instructions apply whenever an agent uses this profile."
            error={errors.systemPrompt}
          >
            <textarea
              {...propsFor('systemPrompt')}
              className="profile-textarea"
              rows={8}
              value={draft.content.systemPrompt}
              onChange={(event) => content({ systemPrompt: event.target.value })}
            />
          </Field>
        </section>
        <section className="profile-editor__band">
          <h2>Sharing</h2>
          <Field
            id="profile-visibility"
            label="Visibility"
            help="Public profiles must have a published revision; that version can be used and forked by other people."
          >
            <Picker
              id="profile-visibility"
              label="Visibility"
              value={editor.profile?.visibility ?? 'private'}
              options={[
                { value: 'private', label: 'Private' },
                {
                  value: 'public',
                  label: 'Public',
                  unavailable:
                    editor.profile?.latestRevision === undefined
                      ? 'Publish a revision first.'
                      : undefined,
                },
              ]}
              disabled={
                locked || editor.dirty || editor.profile === undefined || editor.failure?.conflict
              }
              onChange={editor.visibility}
            />
          </Field>
        </section>
        <EditorActions
          profile={editor.profile}
          dirty={editor.dirty}
          locked={locked}
          pending={editor.pending}
          readingCurrent={editor.readingCurrent}
          failure={editor.failure}
          success={editor.success}
          errorCount={Object.keys(errors).length}
          onPublish={editor.publish}
          onRetire={editor.retire}
          onRetry={editor.retry}
          onLoadCurrent={editor.loadCurrent}
        />
      </form>
    </section>
  );
}
