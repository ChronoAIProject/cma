import { useEffect, useState } from 'react';
import type { ResourcePage } from '../../api/types';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { IconPlusOutline16, IconSearchOutline16, IconTrashOutline16 } from '../../ui/icons';
import type { SkillCatalogClient, SkillCatalogEntry, SkillReference } from '../client';

interface SearchState extends ResourcePage<SkillCatalogEntry> {
  readonly loading: boolean;
  readonly error?: string;
}

export function SkillPicker({
  catalog,
  value,
  onChange,
  disabled,
}: {
  readonly catalog: SkillCatalogClient;
  readonly value: readonly SkillReference[];
  readonly onChange: (skills: readonly SkillReference[]) => void;
  readonly disabled: boolean;
}) {
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState<string>();
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<SearchState>({ items: [], loading: true });
  useEffect(() => {
    const abort = new AbortController();
    const timer = setTimeout(() => {
      void catalog
        .searchSkills(query, { cursor, signal: abort.signal })
        .then((page) => {
          if (abort.signal.aborted) return;
          if (page.next !== undefined && page.next === cursor)
            throw new Error('Nonadvancing skill page');
          setState((previous) => ({
            items:
              cursor === undefined
                ? page.items
                : [
                    ...previous.items,
                    ...page.items.filter(
                      (item) => !previous.items.some((entry) => entry.id === item.id),
                    ),
                  ],
            next: page.next,
            loading: false,
          }));
        })
        .catch(() => {
          if (!abort.signal.aborted)
            setState((previous) => ({
              ...previous,
              loading: false,
              error: 'Skills could not be loaded.',
            }));
        });
    }, 200);
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
  }, [catalog, query, cursor, reload]);
  return (
    <div className="skill-picker">
      <label className="sr-only" htmlFor="profile-skill-search">
        Search Ornn skills
      </label>
      <Input
        id="profile-skill-search"
        placement="skill-picker__search"
        aria-describedby="profile-skill-search-help"
        icon={<IconSearchOutline16 />}
        placeholder="Search Ornn skills"
        value={query}
        disabled={disabled}
        onChange={(event) => {
          setQuery(event.target.value);
          setCursor(undefined);
          setState({ items: [], loading: true });
        }}
      />
      <p id="profile-skill-search-help" className="profile-field__help">
        Search Ornn, then choose the skills and versions this agent should use.
      </p>
      {value.length === 0 ? (
        <p className="profile-field__help">No skills selected.</p>
      ) : (
        <ul className="skill-picker__selected">
          {value.map((skill, index) => (
            <li key={`${skill.id}:${String(index)}`}>
              <span>{state.items.find((entry) => entry.id === skill.id)?.name ?? skill.id}</span>
              <div>
                <label className="sr-only" htmlFor={`profile-skill-version-${String(index)}`}>
                  Version for {skill.id}
                </label>
                <Input
                  id={`profile-skill-version-${String(index)}`}
                  aria-describedby={`profile-skill-version-${String(index)}-help`}
                  value={skill.version ?? ''}
                  placeholder="Latest available"
                  disabled={disabled}
                  onChange={(event) =>
                    onChange(
                      value.map((current, at) =>
                        at === index
                          ? { ...current, version: event.target.value.trim() || undefined }
                          : current,
                      ),
                    )
                  }
                />
                <Button
                  size="sm"
                  icon={<IconTrashOutline16 />}
                  aria-label={`Remove ${skill.id}`}
                  title={`Remove ${skill.id}`}
                  disabled={disabled}
                  onClick={() => onChange(value.filter((_, at) => at !== index))}
                />
              </div>
              <p
                id={`profile-skill-version-${String(index)}-help`}
                className="profile-field__help skill-picker__version-help"
              >
                Leave this empty to use the latest available version.
              </p>
            </li>
          ))}
        </ul>
      )}
      <ul className="skill-picker__results" aria-label="Ornn skills" aria-busy={state.loading}>
        {state.items.map((entry) => {
          const selected = value.some((skill) => skill.id === entry.id);
          return (
            <li key={entry.id}>
              <div>
                <strong>{entry.name}</strong>
                {entry.description === undefined ? null : <p>{entry.description}</p>}
                <span>
                  {entry.id}
                  {entry.versions[0] === undefined ? '' : ` · ${entry.versions[0]}`}
                </span>
              </div>
              <Button
                size="sm"
                icon={<IconPlusOutline16 />}
                aria-label={`Add ${entry.name}`}
                title={
                  selected
                    ? 'Already selected'
                    : value.length >= 32
                      ? 'At most 32 skills'
                      : `Add ${entry.name}`
                }
                disabled={disabled || selected || value.length >= 32}
                onClick={() =>
                  onChange([...value, { source: 'ornn', id: entry.id, version: entry.versions[0] }])
                }
              />
            </li>
          );
        })}
      </ul>
      {state.loading ? <p role="status">Loading skills</p> : null}
      {state.error !== undefined ? (
        <div role="alert">
          <p>{state.error}</p>
          <Button
            size="sm"
            onClick={() => {
              setState((previous) => ({ ...previous, loading: true, error: undefined }));
              setReload((version) => version + 1);
            }}
          >
            Try again
          </Button>
        </div>
      ) : !state.loading && state.items.length === 0 ? (
        <p>No matching skills</p>
      ) : null}
      {state.next === undefined ? null : (
        <Button
          size="sm"
          disabled={state.loading}
          onClick={() => {
            setCursor(state.next);
            setState((previous) => ({ ...previous, loading: true }));
          }}
        >
          More skills
        </Button>
      )}
    </div>
  );
}
