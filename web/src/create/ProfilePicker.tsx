import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { ProfileKind } from '../routing/routes';
import { ChoiceGroup } from '../profiles/ChoiceGroup';
import { Button } from '../ui/Button';
import { IconCheckOutline16, IconPlusOutline16 } from '../ui/icons';
import { ReferenceLink } from '../profiles/ReferenceLink';
import type { CreationClient, CreationProfile, ProfileChoice, ProfileSource } from './client';
import { ProfilePreview } from './ProfilePreview';

export function ProfilePicker({
  client,
  kind,
  value,
  onChange,
  disabled = false,
  personalOnly = false,
}: {
  readonly client: CreationClient;
  readonly kind: ProfileKind;
  readonly value?: CreationProfile;
  readonly onChange: (value: CreationProfile | undefined) => void;
  readonly disabled?: boolean;
  readonly personalOnly?: boolean;
}) {
  const [source, setSource] = useState<ProfileSource>('own');
  const [choices, setChoices] = useState<readonly ProfileChoice[]>([]);
  const [next, setNext] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [reload, setReload] = useState(0);
  const active = useRef<AbortController | undefined>(undefined);
  const version = useRef(0);
  useEffect(() => {
    const request = new AbortController();
    const current = ++version.current;
    active.current?.abort();
    active.current = request;
    void client
      .listProfiles(kind, source, { signal: request.signal })
      .then((page) => {
        if (request.signal.aborted || current !== version.current) return;
        setChoices(page.items);
        setNext(page.next);
        setError(undefined);
        setLoading(false);
      })
      .catch(() => {
        if (!request.signal.aborted) {
          setError('Profiles could not be loaded.');
          setLoading(false);
        }
      });
    return () => request.abort();
  }, [client, kind, source, reload]);
  useEffect(() => () => active.current?.abort(), []);
  const choose = async (choice: ProfileChoice) => {
    if (disabled) return;
    active.current?.abort();
    const request = new AbortController();
    active.current = request;
    onChange(undefined);
    setLoading(true);
    setError(undefined);
    try {
      const selected = await client.readProfile(kind, choice, { signal: request.signal });
      if (!request.signal.aborted) onChange(selected);
    } catch {
      if (!request.signal.aborted)
        setError('This published version could not be loaded. Choose the profile again.');
    } finally {
      if (!request.signal.aborted) setLoading(false);
    }
  };
  const more = async () => {
    if (next === undefined || loading) return;
    const cursor = next;
    const request = new AbortController();
    active.current = request;
    setLoading(true);
    try {
      const page = await client.listProfiles(kind, source, { cursor, signal: request.signal });
      if (request.signal.aborted) return;
      if (page.next === cursor) throw new Error('Pagination did not advance.');
      setChoices((current) => [
        ...current,
        ...page.items.filter(
          (item) => !current.some((old) => old.id === item.id && old.revision === item.revision),
        ),
      ]);
      setNext(page.next);
    } catch {
      if (!request.signal.aborted) setError('More profiles could not be loaded.');
    } finally {
      if (!request.signal.aborted) setLoading(false);
    }
  };
  return (
    <div className="create-profile-picker">
      {personalOnly ? (
        <p>Your published profiles</p>
      ) : (
        <ChoiceGroup
          label="Profile source"
          value={source}
          options={[
            { value: 'own', label: 'My profiles' },
            { value: 'public', label: 'Public' },
            { value: 'default', label: 'Default' },
          ]}
          disabled={disabled}
          onChange={(source) => {
            setSource(source);
            setChoices([]);
            setNext(undefined);
            setLoading(true);
            setError(undefined);
          }}
        />
      )}
      {loading ? <p role="status">Loading profiles</p> : null}
      <Presence>
        {error === undefined ? null : (
          <div role="alert">
            <p>{error}</p>
            <Button
              disabled={disabled || loading}
              onClick={() => {
                setLoading(true);
                setReload((value) => value + 1);
              }}
            >
              Try again
            </Button>
          </div>
        )}
      </Presence>
      <ul className="create-profile-picker__list">
        {choices.map((choice) => (
          <ProfilePreview
            key={`${source}:${choice.id}:${String(choice.revision)}`}
            client={client}
            kind={kind}
            choice={choice}
          >
            <span>
              <ReferenceLink kind={kind} id={choice.id} name={choice.name} />{' '}
              <span>Version {choice.revision}</span>
            </span>
            <Button
              variant="outline"
              aria-label={`Select ${choice.name} Version ${choice.revision}`}
              title={`Select ${choice.name}`}
              disabled={disabled || loading}
              aria-pressed={
                value?.revision.profileId === choice.id &&
                value.revision.revision === choice.revision
              }
              icon={
                value?.revision.profileId === choice.id &&
                value.revision.revision === choice.revision ? (
                  <IconCheckOutline16 />
                ) : (
                  <IconPlusOutline16 />
                )
              }
              onClick={() => void choose(choice)}
            />
          </ProfilePreview>
        ))}
      </ul>
      {!loading && !error && choices.length === 0 ? <p>No published profiles</p> : null}
      {next === undefined ? null : (
        <Button disabled={disabled || loading} onClick={() => void more()}>
          Load more
        </Button>
      )}
      {value === undefined ? null : (
        <div className="create-profile-picker__selection">
          <strong>
            <ReferenceLink kind={kind} id={value.revision.profileId} name={value.name} />
          </strong>
          <span>Version {value.revision.revision}</span>
          {value.kind === 'agent' ? (
            <p>
              {value.revision.content.model ?? 'Deployment model'} ·{' '}
              {value.revision.content.thinkingEffort === 'x_high'
                ? 'Extra high'
                : value.revision.content.thinkingEffort}{' '}
              effort
            </p>
          ) : null}
          {value.kind === 'sandbox' ? (
            <p>
              {value.revision.content.cpuMillis / 1000} CPU cores ·{' '}
              {value.revision.content.memoryMib / 1024} GB ·{' '}
              {value.revision.content.modelAccess === 'own_credentials'
                ? 'Your own account'
                : 'ChronoAI account'}
            </p>
          ) : null}
          {value.kind === 'workspace' ? (
            <p>
              {value.revision.content.location.type !== 'directory'
                ? value.revision.content.location.url
                : value.revision.content.location.name}
            </p>
          ) : null}
        </div>
      )}
    </div>
  );
}
