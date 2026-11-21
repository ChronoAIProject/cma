import { useEffect, useState } from 'react';
import { Input } from './Input';
import { Button } from './Button';
import './ShareRecipientSearch.css';

type Identity = { readonly subject: string; readonly display_name: string | null };
type SearchReply<Person> = { readonly text: string; readonly people?: readonly Person[] };
const name = (person: Identity) => person.display_name?.trim() || 'Name unavailable';

export function ShareRecipientSearch<Person extends Identity>({
  search,
  value,
  onChange,
  disabled,
  single = false,
}: {
  readonly search: (query: string, signal: AbortSignal) => Promise<readonly Person[]>;
  readonly value: readonly Person[];
  readonly onChange: (value: readonly Person[]) => void;
  readonly disabled: boolean;
  readonly single?: boolean;
}) {
  const [query, setQuery] = useState('');
  const [reply, setReply] = useState<SearchReply<Person>>();
  const text = query.trim();
  const length = new TextEncoder().encode(text).length;
  const searchable = length >= 2 && length <= 128;
  const current = reply?.text === text ? reply : undefined;
  const results = current?.people ?? [];
  const loading = searchable && !current;
  const error = current && !current.people;
  useEffect(() => {
    if (!searchable) return;
    const request = new AbortController();
    const timer = setTimeout(() => {
      void search(text, request.signal).then(
        (people) => {
          if (!request.signal.aborted) setReply({ text, people });
        },
        () => {
          if (!request.signal.aborted) setReply({ text });
        },
      );
    }, 250);
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [search, searchable, text]);
  const identity = (person: Person, fallback = 'Name unavailable') => (
    <span className={single ? 'share-recipient-search__identity' : undefined}>
      {person.display_name?.trim() || fallback}
      {single ? <small>{person.subject}</small> : null}
    </span>
  );
  return (
    <div className="share-recipient-search">
      <label>
        <span>{single ? 'Find people by name or NyxID user ID' : 'Find people by username'}</span>
        <Input
          aria-label="Find share recipients"
          value={query}
          disabled={disabled}
          maxLength={128}
          onChange={(event) => {
            const next = event.target.value;
            if (single && value.length) onChange([]);
            setQuery(next);
            if (next.trim() !== text) setReply(undefined);
          }}
        />
      </label>
      {value.length ? (
        <ul className="share-recipient-search__list" aria-label="Selected recipients">
          {value.map((person) => (
            <li key={person.subject}>
              {identity(person, single ? undefined : 'Selected person')}
              <Button
                size="sm"
                disabled={disabled}
                aria-label={`Remove ${person.display_name?.trim() || 'person'}`}
                onClick={() => onChange(value.filter((item) => item.subject !== person.subject))}
              >
                Remove
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
      {loading ? <p role="status">Finding people…</p> : null}
      {error ? <p role="alert">People could not be loaded. Adjust the search to retry.</p> : null}
      {!loading && searchable && results.length === 0 && !error ? <p>No matching people.</p> : null}
      {results.length && (!single || !value.length) ? (
        <ul className="share-recipient-search__list" aria-label="Matching people">
          {results.map((person) => {
            const selected = value.some((entry) => entry.subject === person.subject);
            return (
              <li key={person.subject}>
                {identity(person)}
                <Button
                  size="sm"
                  disabled={disabled || selected}
                  aria-label={single ? `Select ${name(person)} (${person.subject})` : undefined}
                  onClick={() => onChange(single ? [person] : [...value, person])}
                >
                  {single ? 'Select' : selected ? 'Added' : 'Add'}
                </Button>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
