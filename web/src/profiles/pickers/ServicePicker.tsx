import { Presence } from '../../ui/Presence';
import { useEffect, useId, useState } from 'react';
import { CatalogFailureNotice } from '../CatalogFailureNotice';
import { catalogFailure, type CatalogFailure } from '../catalogFailure';
import { Input } from '../../ui/Input';
import { IconSearchOutline16 } from '../../ui/icons';
import type { ServiceCatalogClient, ServiceCatalogEntry } from '../client';
import { ReferenceLink } from '../ReferenceLink';
import { isCodexConnection } from '../modelProvider';
import '../Profiles.css';

export function ServicePicker({
  catalog,
  value,
  onChange,
  disabled,
  fixed = [],
  help = 'Choose the services this sandbox needs. We’ll help each user connect any missing services before opening their sandbox.',
}: {
  readonly catalog?: ServiceCatalogClient;
  readonly value: readonly string[];
  readonly onChange: (value: readonly string[]) => void;
  readonly disabled: boolean;
  readonly help?: string;
  readonly fixed?: readonly { readonly id: string; readonly reason: string }[];
}) {
  const searchId = useId();
  const helpId = useId();
  const [query, setQuery] = useState('');
  const searchTerm = query.trim().toLocaleLowerCase();
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<{
    catalog: ServiceCatalogClient | undefined;
    entries: readonly ServiceCatalogEntry[];
    loading: boolean;
    error?: CatalogFailure;
  }>({ catalog, entries: [], loading: catalog !== undefined });
  if (state.catalog !== catalog) {
    setState({ catalog, entries: [], loading: catalog !== undefined });
    setQuery('');
  }
  const unavailable = catalog === undefined || state.loading || state.error !== undefined;
  useEffect(() => {
    if (catalog === undefined) return;
    const request = new AbortController();
    void catalog
      .listServices({ signal: request.signal })
      .then((entries) => {
        if (!request.signal.aborted) setState({ catalog, entries, loading: false });
      })
      .catch((error: unknown) => {
        if (!request.signal.aborted)
          setState({ catalog, entries: [], loading: false, error: catalogFailure(error) });
      });
    return () => request.abort();
  }, [catalog, reload]);
  return (
    <div className="profile-field">
      <p id={helpId} className="profile-field__help">
        {help}
      </p>
      <label className="sr-only" htmlFor={searchId}>
        Search services
      </label>
      <Input
        id={searchId}
        value={query}
        icon={<IconSearchOutline16 />}
        aria-describedby={helpId}
        disabled={disabled || unavailable}
        onChange={(event) => setQuery(event.target.value)}
      />
      <ul className="profile-service-list" aria-label="Services">
        {[
          ...fixed.map((entry) => ({
            id: entry.id,
            name: entry.id,
            description: entry.reason,
            catalogId: undefined,
            detailUrl: undefined,
          })),
          ...state.entries.filter((entry) => !fixed.some((item) => item.id === entry.id)),
          ...value
            .filter(
              (id) =>
                !state.entries.some((entry) => entry.id === id) &&
                !fixed.some((entry) => entry.id === id),
            )
            .map((id) => ({
              id,
              name: 'Service unavailable',
              catalogId: undefined,
              detailUrl: undefined,
            })),
        ]
          .filter(
            (entry) => fixed.some((item) => item.id === entry.id) || !isCodexConnection(entry.id),
          )
          .filter((entry) => entry.name.toLocaleLowerCase().includes(searchTerm))
          .map((entry) => (
            <li key={entry.id}>
              <label>
                <input
                  type="checkbox"
                  checked={value.includes(entry.id) || fixed.some((item) => item.id === entry.id)}
                  disabled={disabled || unavailable || fixed.some((item) => item.id === entry.id)}
                  onChange={(event) =>
                    onChange(
                      event.target.checked
                        ? [...value, entry.id]
                        : value.filter((id) => id !== entry.id),
                    )
                  }
                />
                <span
                  title={[entry.name, 'description' in entry ? entry.description : undefined]
                    .filter(Boolean)
                    .join(' — ')}
                >
                  <ReferenceLink
                    kind="service"
                    id={entry.id}
                    name={entry.name}
                    catalogId={entry.catalogId}
                    detailUrl={entry.detailUrl}
                  />{' '}
                  {'description' in entry && entry.description !== undefined ? (
                    <small> {entry.description}</small>
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
            <CatalogFailureNotice
              failure={state.error}
              disabled={disabled}
              onRetry={() => {
                setState({ catalog, entries: [], loading: true });
                setReload((version) => version + 1);
              }}
            />
          ) : null}
        </Presence>
      </>
    </div>
  );
}
