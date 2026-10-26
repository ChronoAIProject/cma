import { useEffect, useState } from 'react';
import type { ShareIdentity } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { Input } from '../ui/Input';
import { Button } from '../ui/Button';

export function AgentShareRecipients({
  client,
  agentId,
  value,
  onChange,
  disabled,
}: {
  readonly client: ResourceViewsClient;
  readonly agentId: string;
  readonly value: readonly ShareIdentity[];
  readonly onChange: (value: readonly ShareIdentity[]) => void;
  readonly disabled: boolean;
}) {
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<readonly ShareIdentity[]>([]);
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    const request = new AbortController();
    if (query.trim().length < 2) {
      return () => request.abort();
    }
    const timer = setTimeout(() => {
      void client.resourceAccess
        .shareRecipients(agentId, query.trim(), { signal: request.signal })
        .then(
          ({ items }) => {
            if (!request.signal.aborted) {
              setResults(items);
              setLoading(false);
            }
          },
          () => {
            if (!request.signal.aborted) {
              setError('People could not be loaded. Adjust the search to retry.');
              setLoading(false);
            }
          },
        );
    }, 250);
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [client, agentId, query]);
  return (
    <div className="agent-share__people">
      <label>
        <span>Find people by username</span>
        <Input
          aria-label="Find share recipients"
          value={query}
          disabled={disabled}
          maxLength={255}
          onChange={(event) => {
            setQuery(event.target.value);
            setResults([]);
            setError(undefined);
            setLoading(event.target.value.trim().length >= 2);
          }}
        />
      </label>
      {value.length ? (
        <ul className="agent-share__selection" aria-label="Selected recipients">
          {value.map((person) => (
            <li key={person.subject}>
              <span>{person.display_name?.trim() || 'Selected person'}</span>
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
      {error ? <p role="alert">{error}</p> : null}
      {!loading && query.trim().length >= 2 && results.length === 0 && !error ? (
        <p>No matching people.</p>
      ) : null}
      {results.length ? (
        <ul className="agent-share__selection" aria-label="Matching people">
          {results.map((person) => (
            <li key={person.subject}>
              <span>{person.display_name?.trim() || 'Name unavailable'}</span>
              <Button
                size="sm"
                disabled={disabled || value.some((entry) => entry.subject === person.subject)}
                onClick={() => onChange([...value, person])}
              >
                {value.some((entry) => entry.subject === person.subject) ? 'Added' : 'Add'}
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
