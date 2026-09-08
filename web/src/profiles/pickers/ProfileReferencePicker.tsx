import { useEffect, useId, useRef, useState } from 'react';
import type { ProfileReference, ResourcePage } from '../../api/types';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { IconPlusOutline16, IconSearchOutline16 } from '../../ui/icons';
import type { ProfilesClient } from '../client';
import type { MarketplaceClient } from '../marketplace/client';
import { ChoiceGroup } from '../ChoiceGroup';
import { moveItem, OrderedActions } from './OrderedActions';
import { ReferenceLink } from '../ReferenceLink';

interface Choice extends ProfileReference {
  readonly name: string;
}

export function ProfileReferencePicker({
  kind,
  profiles,
  marketplace,
  value,
  onChange,
  maximum,
  disabled,
  error,
}: {
  readonly kind: 'agent' | 'workspace';
  readonly profiles: ProfilesClient;
  readonly marketplace: Pick<MarketplaceClient, 'listMarketplace'>;
  readonly value: readonly ProfileReference[];
  readonly onChange: (value: readonly ProfileReference[]) => void;
  readonly maximum: number;
  readonly disabled: boolean;
  readonly error?: string;
}) {
  const id = useId();
  const [source, setSource] = useState<'own' | 'public'>('own');
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState<string>();
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<ResourcePage<Choice> & { loading: boolean; error?: string }>({
    items: [],
    loading: true,
  });
  const [names, setNames] = useState<ReadonlyMap<string, string>>(new Map());
  const seen = useRef(new Set<string>());
  useEffect(() => {
    const request = new AbortController();
    const timer = setTimeout(() => {
      const page =
        source === 'own'
          ? profiles.listProfiles(kind, { cursor, signal: request.signal }).then((result) => ({
              next: result.next,
              items: result.items
                .filter(
                  (entry) => entry.latestRevision !== undefined && entry.retiredAt === undefined,
                )
                .map((entry) => ({
                  id: entry.id,
                  name: entry.name,
                  revision: entry.latestRevision?.revision ?? 0,
                })),
            }))
          : marketplace
              .listMarketplace(kind, query, { cursor, signal: request.signal })
              .then((result) => ({
                next: result.next,
                items: result.items.map((entry) => ({
                  id: entry.id,
                  name: entry.name,
                  revision: entry.latestRevision,
                })),
              }));
      void page
        .then((result) => {
          if (request.signal.aborted) return;
          if (
            result.next !== undefined &&
            (result.next === cursor || seen.current.has(result.next))
          )
            throw new Error('Nonadvancing profile page');
          if (result.next !== undefined) seen.current.add(result.next);
          setNames((previous) => {
            const next = new Map(previous);
            result.items.forEach((entry) => next.set(entry.id, entry.name));
            return next;
          });
          setState((previous) => ({
            items:
              cursor === undefined
                ? result.items
                : [
                    ...previous.items,
                    ...result.items.filter(
                      (entry) => !previous.items.some((item) => item.id === entry.id),
                    ),
                  ],
            next: result.next,
            loading: false,
          }));
        })
        .catch(() => {
          if (!request.signal.aborted)
            setState((previous) => ({
              ...previous,
              loading: false,
              error: 'Profiles could not be loaded.',
            }));
        });
    }, 200);
    return () => {
      clearTimeout(timer);
      request.abort();
    };
  }, [profiles, marketplace, kind, source, cursor, query, reload]);
  const reset = () => {
    seen.current.clear();
    setCursor(undefined);
    setState({ items: [], loading: true });
  };
  const entries = state.items.filter(
    (entry) =>
      source === 'public' || entry.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()),
  );
  return (
    <div className="profile-field">
      <p id={`${id}-help`} className="profile-field__help">
        Choose up to {maximum} published {kind} profiles; they start in the order shown.
      </p>
      <ol className="profile-ordered-list">
        {value.map((entry, index) => (
          <li key={`${entry.id}:${String(entry.revision)}:${String(index)}`}>
            <span>
              <ReferenceLink kind={kind} id={entry.id} name={names.get(entry.id)} /> · Revision{' '}
              {entry.revision}
            </span>
            <OrderedActions
              label={`${kind} profile ${String(index + 1)}`}
              index={index}
              length={value.length}
              disabled={disabled}
              onMove={(to) => onChange(moveItem(value, index, to))}
              onRemove={() => onChange(value.filter((_, at) => at !== index))}
            />
          </li>
        ))}
      </ol>
      <ChoiceGroup
        label={`${kind === 'agent' ? 'Agent' : 'Workspace'} profile source`}
        options={[
          { value: 'own', label: 'My profiles' },
          { value: 'public', label: 'Public profiles' },
        ]}
        value={source}
        disabled={disabled}
        onChange={(next) => {
          setSource(next);
          reset();
        }}
      />
      <label className="sr-only" htmlFor={id}>
        Search {kind} profiles
      </label>
      <Input
        id={id}
        icon={<IconSearchOutline16 />}
        value={query}
        aria-describedby={`${id}-help`}
        disabled={disabled}
        onChange={(event) => {
          setQuery(event.target.value);
          reset();
        }}
      />
      <ul
        className="profile-picker-results"
        aria-label={`Available ${kind} profiles`}
        aria-busy={state.loading}
      >
        {entries.map((entry) => (
          <li key={entry.id}>
            <span>
              <ReferenceLink kind={kind} id={entry.id} name={entry.name} /> · Revision{' '}
              {entry.revision}
            </span>
            <Button
              type="button"
              size="sm"
              icon={<IconPlusOutline16 />}
              title={`Add ${entry.name}`}
              aria-label={`Add ${entry.name}`}
              disabled={
                disabled ||
                value.length >= maximum ||
                value.some(
                  (selected) => selected.id === entry.id && selected.revision === entry.revision,
                )
              }
              onClick={() => onChange([...value, { id: entry.id, revision: entry.revision }])}
            />
          </li>
        ))}
      </ul>
      {state.loading ? <p role="status">Loading profiles</p> : null}
      {state.error === undefined ? (
        !state.loading && entries.length === 0 ? (
          <p>No matching published profiles</p>
        ) : null
      ) : (
        <div role="alert">
          <p>{state.error}</p>
          <Button
            type="button"
            onClick={() => {
              setState((previous) => ({ ...previous, loading: true, error: undefined }));
              setReload((version) => version + 1);
            }}
          >
            Try again
          </Button>
        </div>
      )}
      {state.next === undefined ? null : (
        <Button
          type="button"
          disabled={state.loading}
          onClick={() => {
            setCursor(state.next);
            setState((previous) => ({ ...previous, loading: true }));
          }}
        >
          More profiles
        </Button>
      )}
      {error === undefined ? null : <p className="profile-field__error">{error}</p>}
    </div>
  );
}
