import { Picker } from '../ui/Picker';
import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { ProfileKind, ProfileReference } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconRefreshOutline16 } from '../ui/icons';
import type { PublisherProfile, PublisherProfileChoice, TriggerPublisherClient } from './client';
import { profileSummary } from './model';
import { ReferenceLink } from '../profiles/ReferenceLink';

export function ProfilePicker({
  client,
  kind,
  value,
  references = [],
  error,
  disabled,
  onSelect,
}: {
  readonly client: TriggerPublisherClient;
  readonly kind: ProfileKind;
  readonly value?: PublisherProfile;
  readonly references?: readonly ProfileReference[];
  readonly error?: string;
  readonly disabled: boolean;
  readonly onSelect: (choice: PublisherProfileChoice) => void;
}) {
  const [source, setSource] = useState<'own' | 'public' | 'sandbox'>(
    kind === 'workspace' && references.length > 0 ? 'sandbox' : 'own',
  );
  const [choices, setChoices] = useState<readonly PublisherProfileChoice[]>([]);
  const [next, setNext] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [loadedKey, setLoadedKey] = useState('');
  const [failure, setFailure] = useState<string>();
  const [reload, setReload] = useState(0);
  const pagination = useRef<AbortController | undefined>(undefined);
  const selectedSource = source === 'sandbox' && references.length === 0 ? 'own' : source;
  const referencesKey = JSON.stringify(references);
  const requestKey = `${selectedSource}:${referencesKey}:${String(reload)}`;
  const busy = loading || loadedKey !== requestKey;
  useEffect(() => {
    const request = new AbortController();
    const read = async () => {
      if (selectedSource === 'sandbox') {
        const refs = JSON.parse(referencesKey) as readonly ProfileReference[];
        const items = await Promise.all(
          refs.map((reference) => client.readProfile(kind, reference, { signal: request.signal })),
        );
        return { items, next: undefined };
      }
      return client.listProfiles(kind, selectedSource, { signal: request.signal });
    };
    void read()
      .then((page) => {
        if (request.signal.aborted) return;
        setChoices(page.items);
        setNext(page.next);
        setLoading(false);
        setLoadedKey(requestKey);
        setFailure(undefined);
      })
      .catch(() => {
        if (!request.signal.aborted) {
          setFailure('Profile choices could not be loaded.');
          setLoading(false);
          setLoadedKey(requestKey);
        }
      });
    return () => {
      request.abort();
      pagination.current?.abort();
    };
  }, [client, kind, selectedSource, referencesKey, requestKey]);
  const loadMore = async () => {
    if (next === undefined || selectedSource === 'sandbox' || loading) return;
    const cursor = next;
    const request = new AbortController();
    pagination.current = request;
    setLoading(true);
    try {
      const page = await client.listProfiles(kind, selectedSource, {
        cursor,
        signal: request.signal,
      });
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
      if (!request.signal.aborted) setFailure('More profile choices could not be loaded.');
    } finally {
      if (!request.signal.aborted) setLoading(false);
    }
  };
  const id = `publisher-${kind}`;
  const current = value === undefined ? '' : `${value.id}:${String(value.revision)}`;
  return (
    <div className="trigger-field">
      <label htmlFor={id}>{kind} profile</label>
      <div className="trigger-picker__controls">
        <Picker
          label={`${kind} profile source`}
          value={selectedSource}
          disabled={disabled}
          onChange={(choice) => {
            setSource(choice as typeof source);
            setChoices([]);
            setNext(undefined);
          }}
          options={[
            ...(kind === 'workspace' && references.length > 0
              ? [{ value: 'sandbox', label: 'From Sandbox profile' }]
              : []),
            { value: 'own', label: 'My profiles' },
            { value: 'public', label: 'Public profiles' },
          ]}
        />
        <Picker
          id={id}
          value={current}
          disabled={disabled || busy}
          describedBy={`${id}-summary${error === undefined ? '' : ` ${id}-error`}`}
          invalid={error !== undefined}
          onChange={(choice) => {
            const chosen = choices.find((item) => `${item.id}:${String(item.revision)}` === choice);
            if (chosen !== undefined) onSelect(chosen);
          }}
          label={`${kind} profile`}
          options={[
            {
              value: '',
              label: busy ? 'Loading profiles' : 'Choose a published revision',
              unavailable: 'Choose a published revision below.',
            },
            ...(value !== undefined &&
            !choices.some((item) => item.id === value.id && item.revision === value.revision)
              ? [
                  {
                    value: current,
                    label: `${value.name} / Revision ${value.revision}`,
                    unavailable: 'This revision is not in the current catalog.',
                  },
                ]
              : []),
            ...choices.map((choice) => ({
              value: `${choice.id}:${String(choice.revision)}`,
              label: `${choice.name} / Revision ${choice.revision}${choice.public ? '' : ' / Private'}`,
            })),
          ]}
        />
      </div>
      {value === undefined ? null : (
        <p>
          <ReferenceLink kind={kind} id={value.id} name={value.name} />
        </p>
      )}
      <p id={`${id}-summary`} className="trigger-caption">
        {value === undefined ? 'No revision selected' : profileSummary(value)}
      </p>
      <Presence>
        {error === undefined ? null : (
          <p id={`${id}-error`} className="trigger-error" role="alert">
            {error}
          </p>
        )}
      </Presence>
      <Presence>
        {failure === undefined ? null : (
          <div role="alert">
            <p>{failure}</p>
            <Button
              icon={<IconRefreshOutline16 />}
              disabled={disabled || loading}
              onClick={() => setReload((current) => current + 1)}
            >
              Try again
            </Button>
          </div>
        )}
      </Presence>
      {next === undefined ? null : (
        <Button disabled={disabled || loading} onClick={() => void loadMore()}>
          Load more
        </Button>
      )}
    </div>
  );
}
