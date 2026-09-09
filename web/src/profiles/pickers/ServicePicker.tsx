import { Presence } from '../../ui/Presence';
import { useEffect, useState } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { IconSearchOutline16 } from '../../ui/icons';
import type { ServiceCatalogClient, ServiceCatalogEntry } from '../client';
import { ReferenceLink } from '../ReferenceLink';

export function ServicePicker({
  catalog,
  value,
  onChange,
  disabled,
}: {
  readonly catalog?: ServiceCatalogClient;
  readonly value: readonly string[];
  readonly onChange: (value: readonly string[]) => void;
  readonly disabled: boolean;
}) {
  const [query, setQuery] = useState('');
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<{
    entries: readonly ServiceCatalogEntry[];
    loading: boolean;
    error?: string;
  }>({ entries: [], loading: catalog !== undefined });
  useEffect(() => {
    if (catalog === undefined) return;
    const request = new AbortController();
    void catalog
      .listServices({ signal: request.signal })
      .then((entries) => {
        if (!request.signal.aborted) setState({ entries, loading: false });
      })
      .catch(() => {
        if (!request.signal.aborted)
          setState({ entries: [], loading: false, error: 'Services could not be loaded.' });
      });
    return () => request.abort();
  }, [catalog, reload]);
  return (
    <div className="profile-field">
      <p id="profile-services-help" className="profile-field__help">
        Select the connected services this sandbox may reach; access is granted only when it starts.
      </p>
      <label className="sr-only" htmlFor="profile-service-search">
        Search services
      </label>
      <Input
        id="profile-service-search"
        value={query}
        icon={<IconSearchOutline16 />}
        aria-describedby="profile-services-help"
        disabled={disabled || catalog === undefined}
        onChange={(event) => setQuery(event.target.value)}
      />
      <ul className="profile-service-list">
        {[
          ...state.entries,
          ...value
            .filter((id) => !state.entries.some((entry) => entry.id === id))
            .map((id) => ({ id, name: 'Service unavailable', catalogId: undefined })),
        ]
          .filter((entry) => entry.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()))
          .map((entry) => (
            <li key={entry.id}>
              <label>
                <input
                  type="checkbox"
                  checked={value.includes(entry.id)}
                  disabled={disabled}
                  onChange={(event) =>
                    onChange(
                      event.target.checked
                        ? [...value, entry.id]
                        : value.filter((id) => id !== entry.id),
                    )
                  }
                />
                <span>
                  <ReferenceLink
                    kind="service"
                    id={entry.id}
                    name={entry.name}
                    catalogId={entry.catalogId}
                  />
                  {'description' in entry && entry.description !== undefined ? (
                    <small>{entry.description}</small>
                  ) : null}
                </span>
              </label>
            </li>
          ))}
      </ul>
      <>
        {catalog === undefined ? (
          <p role="status">The service catalogue is unavailable.</p>
        ) : state.loading ? (
          <p role="status">Loading services</p>
        ) : state.error !== undefined ? null : state.entries.length === 0 ? (
          <p>No services available</p>
        ) : null}
        <Presence>
          {catalog === undefined ? null : state.loading ? null : state.error !== undefined ? (
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
          ) : null}
        </Presence>
      </>
    </div>
  );
}
