import { Presence } from '../../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { IconBranchOutline16, IconFolderOpenOutline16 } from '../../ui/icons';
import type { RepositoryResolver, WorkspaceProfileContent } from '../client';
import { Field } from '../Field';
import { fieldProps, type EditorState } from '../ProfileEditor';
import { directoryNameError, repositoryKey, repositoryName } from '../workspaceValidation';

export function WorkspaceLocationPicker({
  editor,
  resolver,
  verified,
  onVerified,
}: {
  readonly editor: EditorState<WorkspaceProfileContent>;
  readonly resolver?: RepositoryResolver;
  readonly verified?: string;
  readonly onVerified: (key: string) => void;
}) {
  const { draft, locked, errors } = editor;
  const value = draft.content;
  const location = value.location;
  const key = repositoryKey(location);
  const request = useRef<AbortController | undefined>(undefined);
  const latestEditor = useRef(editor);
  const [checking, setChecking] = useState<string>();
  const [failure, setFailure] = useState<{ key: string; message: string }>();
  useEffect(() => () => request.current?.abort(), [key]);
  useEffect(() => {
    latestEditor.current = editor;
  }, [editor]);
  const content = (next: Partial<WorkspaceProfileContent>) => {
    request.current?.abort();
    setChecking(undefined);
    editor.change({ ...draft, content: { ...value, ...next } });
  };
  const check = async () => {
    if (
      location.type !== 'git_repository' ||
      resolver === undefined ||
      (checking === key && request.current?.signal.aborted === false)
    )
      return;
    const abort = new AbortController();
    request.current = abort;
    setChecking(key);
    setFailure(undefined);
    try {
      const resolved = await resolver.resolveRepository(location, { signal: abort.signal });
      const current = latestEditor.current;
      if (!abort.signal.aborted && repositoryKey(current.draft.content.location) === key) {
        const canonical = { ...location, url: resolved.url };
        onVerified(repositoryKey(canonical));
        current.change({
          ...current.draft,
          content: { ...current.draft.content, location: canonical },
        });
      }
    } catch {
      if (!abort.signal.aborted)
        setFailure({
          key,
          message:
            'The public repository or reference could not be verified. Check the URL and try again.',
        });
    } finally {
      if (!abort.signal.aborted) setChecking(undefined);
    }
  };
  const currentName =
    value.name || (location.type === 'git_repository' ? repositoryName(location.url) : '');
  const nameError =
    errors.directoryName ?? (currentName === '' ? undefined : directoryNameError(currentName));
  return (
    <>
      <div
        className="profile-location-options"
        role="radiogroup"
        aria-label="Location"
        aria-describedby="profile-location-help"
      >
        {(
          [
            { type: 'git_repository', label: 'GitHub repository', icon: <IconBranchOutline16 /> },
            { type: 'directory', label: 'Empty directory', icon: <IconFolderOpenOutline16 /> },
          ] as const
        ).map((option) => (
          <label key={option.type}>
            <input
              type="radio"
              name="workspace-location"
              value={option.type}
              checked={location.type === option.type}
              disabled={locked}
              onChange={() =>
                content({
                  location:
                    option.type === 'directory'
                      ? { type: 'directory', name: currentName || 'workspace' }
                      : { type: 'git_repository', url: '' },
                  name: option.type === 'directory' ? currentName || 'workspace' : currentName,
                })
              }
            />
            <span>
              {option.icon}
              {option.label}
            </span>
          </label>
        ))}
      </div>
      <p id="profile-location-help" className="profile-field__help">
        Start from a GitHub repository or a new empty directory.
      </p>
      {location.type !== 'directory' ? (
        <>
          <Field
            id="profile-repositoryUrl"
            label="Repository URL"
            help="Use the HTTPS address of a public GitHub repository."
            error={errors.repositoryUrl}
          >
            <Input
              {...fieldProps(editor, 'repositoryUrl')}
              type="url"
              value={location.url}
              placeholder="https://github.com/owner/repository"
              onChange={(event) =>
                content({
                  location: { ...location, url: event.target.value },
                  name:
                    value.name === '' || value.name === repositoryName(location.url)
                      ? repositoryName(event.target.value)
                      : value.name,
                })
              }
            />
          </Field>
          <Field
            id="profile-repositoryReference"
            label="Branch or tag"
            help="Leave this empty to use the repository's default branch."
            error={errors.repositoryReference}
          >
            <Input
              {...fieldProps(editor, 'repositoryReference')}
              value={location.reference ?? ''}
              onChange={(event) =>
                content({ location: { ...location, reference: event.target.value || undefined } })
              }
            />
          </Field>
          {resolver === undefined ? (
            <p role="status">Repository verification is unavailable.</p>
          ) : (
            <div className="profile-editor__actions">
              <Button
                type="button"
                disabled={locked || location.url === '' || checking === key}
                onClick={() => void check()}
              >
                {checking === key ? 'Verifying repository' : 'Verify repository'}
              </Button>
              <Presence>
                {verified === key ? <span role="status">Repository verified</span> : null}
              </Presence>
            </div>
          )}
          <Presence>
            {failure?.key === key ? (
              <p role="alert" className="profile-field__error">
                {failure.message}
              </p>
            ) : null}
          </Presence>
        </>
      ) : (
        <Field
          id="profile-locationName"
          label="Empty directory name"
          help="Name the new empty directory that will hold this workspace."
          error={errors.locationName}
        >
          <Input
            {...fieldProps(editor, 'locationName')}
            value={location.name}
            onChange={(event) =>
              content({
                location: { type: 'directory', name: event.target.value },
                name: value.name === location.name ? event.target.value : value.name,
              })
            }
          />
        </Field>
      )}
      <Field
        id="profile-directoryName"
        label="Directory name"
        help="This name is used below the sandbox home directory and must be unique in that sandbox."
        error={nameError}
      >
        <Input
          {...fieldProps(editor, 'directoryName')}
          aria-invalid={nameError === undefined ? undefined : true}
          aria-describedby={`profile-directoryName-help${nameError === undefined ? '' : ' profile-directoryName-error'}`}
          value={value.name}
          placeholder={
            location.type === 'git_repository' ? repositoryName(location.url) : undefined
          }
          onChange={(event) => content({ name: event.target.value })}
        />
      </Field>
    </>
  );
}
