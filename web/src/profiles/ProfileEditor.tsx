import { useEffect, useRef, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import type { ProfileDocument, ProfileDraft, ProfileEditorClient } from './client';
import { useProfileEditor } from './editor';
import { EditorActions } from './EditorActions';
import { Field, fieldDescription } from './Field';
import type { FieldErrors } from './validation';
import './Profiles.css';

export type EditorState<Content> = ReturnType<typeof useProfileEditor<Content>>;

export function fieldProps<Content>(editor: EditorState<Content>, field: string) {
  return {
    id: `profile-${field}`,
    'aria-invalid': editor.errors[field] === undefined ? undefined : (true as const),
    'aria-describedby': fieldDescription(`profile-${field}`, editor.errors[field]),
    disabled: editor.locked,
  };
}

export function ProfileEditor<Content>({
  client,
  profileId,
  initial,
  validate,
  onSaved,
  label,
  children,
}: {
  readonly client: ProfileEditorClient<Content>;
  readonly profileId?: string;
  readonly initial: ProfileDraft<Content>;
  readonly validate: (draft: ProfileDraft<Content>) => FieldErrors;
  readonly onSaved?: (profile: ProfileDocument<Content>) => void;
  readonly label: string;
  readonly children: (editor: EditorState<Content>) => ReactNode;
}) {
  const editor = useProfileEditor(client, profileId, initial, validate, onSaved);
  const form = useRef<HTMLFormElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const title = editor.profile?.name ?? `New ${label.toLowerCase()} profile`;
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
  if (editor.loading)
    return (
      <section className="profile-editor" aria-busy="true">
        <h1>{label} profile</h1>
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
          <p className="profile-editor__eyebrow">{label} profile</p>
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
            help={`Choose a name that identifies this ${label.toLowerCase()} setup.`}
            error={editor.errors.name}
          >
            <Input
              {...fieldProps(editor, 'name')}
              value={editor.draft.name}
              onChange={(event) => editor.change({ ...editor.draft, name: event.target.value })}
            />
          </Field>
          <Field
            id="profile-description"
            label="Description"
            help="Describe when someone should use this profile."
            error={editor.errors.description}
          >
            <textarea
              {...fieldProps(editor, 'description')}
              className="profile-textarea"
              rows={3}
              value={editor.draft.description}
              onChange={(event) =>
                editor.change({ ...editor.draft, description: event.target.value })
              }
            />
          </Field>
        </section>
        {children(editor)}
        <section className="profile-editor__band">
          <h2>Sharing</h2>
          <Field
            id="profile-visibility"
            label="Visibility"
            help="Public profiles need a published revision and public references before other people can use or fork them."
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
                editor.locked ||
                editor.dirty ||
                editor.profile === undefined ||
                editor.failure?.conflict
              }
              onChange={editor.visibility}
            />
          </Field>
        </section>
        <EditorActions
          profile={editor.profile}
          dirty={editor.dirty}
          locked={editor.locked}
          pending={editor.pending}
          readingCurrent={editor.readingCurrent}
          failure={editor.failure}
          success={editor.success}
          errorCount={Object.keys(editor.errors).length}
          onPublish={editor.publish}
          onRetire={editor.retire}
          onRetry={editor.retry}
          onLoadCurrent={editor.loadCurrent}
        />
      </form>
    </section>
  );
}
