import { useEffect, useRef, useState } from 'react';
import type { AppRoute, ProfileKind } from '../../routing/routes';
import { routePath } from '../../routing/routes';
import type { ResourcePage } from '../../api/types';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { IconSearchOutline16 } from '../../ui/icons';
import { ProfileTabs } from '../ProfileTabs';
import type { MarketplaceClient, MarketplaceSummary } from './client';
import '../Profiles.css';
import './Marketplace.css';

export function Marketplace({
  client,
  onNavigate,
  initialKind = 'sandbox',
}: {
  readonly client: Pick<MarketplaceClient, 'listMarketplace'>;
  readonly onNavigate: (route: AppRoute) => void;
  readonly initialKind?: ProfileKind;
}) {
  const [kind, setKind] = useState(initialKind);
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState<string>();
  const [reload, setReload] = useState(0);
  const [state, setState] = useState<
    ResourcePage<MarketplaceSummary> & { loading: boolean; error?: string }
  >({ items: [], loading: true });
  const seen = useRef(new Set<string>());
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    document.title = 'Marketplace · CMA';
    heading.current?.focus();
    return () => {
      document.title = 'CMA';
    };
  }, []);
  useEffect(() => {
    const request = new AbortController();
    const timer = setTimeout(() => {
      void client
        .listMarketplace(kind, query, { cursor, signal: request.signal })
        .then((page) => {
          if (request.signal.aborted) return;
          if (page.next !== undefined && (page.next === cursor || seen.current.has(page.next)))
            throw new Error('Nonadvancing marketplace page');
          if (page.next !== undefined) seen.current.add(page.next);
          setState((previous) => ({
            items:
              cursor === undefined
                ? page.items
                : [
                    ...previous.items,
                    ...page.items.filter(
                      (entry) => !previous.items.some((item) => item.id === entry.id),
                    ),
                  ],
            next: page.next,
            loading: false,
          }));
        })
        .catch((error: unknown) => {
          if (!request.signal.aborted)
            setState((previous) => ({
              ...previous,
              loading: false,
              error:
                typeof error === 'object' &&
                error !== null &&
                'code' in error &&
                error.code === 'marketplace_disabled'
                  ? 'The marketplace is currently unavailable.'
                  : 'Marketplace profiles could not be loaded.',
            }));
        });
    }, 200);
    return () => {
      clearTimeout(timer);
      request.abort();
    };
  }, [client, kind, query, cursor, reload]);
  const reset = () => {
    seen.current.clear();
    setCursor(undefined);
    setState({ items: [], loading: true });
  };
  return (
    <section className="marketplace">
      <header className="profiles-home__header">
        <h1 ref={heading} tabIndex={-1}>
          Marketplace
        </h1>
      </header>
      <div className="profile-field">
        <label className="sr-only" htmlFor="marketplace-search">
          Search marketplace
        </label>
        <Input
          id="marketplace-search"
          value={query}
          icon={<IconSearchOutline16 />}
          placeholder="Search profiles"
          aria-describedby="marketplace-search-help"
          onChange={(event) => {
            setQuery(event.target.value);
            reset();
          }}
        />
        <p id="marketplace-search-help" className="profile-field__help">
          Search the names and descriptions of public profiles.
        </p>
      </div>
      <ProfileTabs
        kind={kind}
        onChange={(next) => {
          if (next !== kind) {
            setKind(next);
            reset();
          }
        }}
      />
      <div
        id="profile-list"
        role="tabpanel"
        aria-labelledby={`profile-tab-${kind}`}
        aria-busy={state.loading}
      >
        <ul className="marketplace-grid">
          {state.items.map((entry) => {
            const route: AppRoute = {
              kind: 'marketplace_profile',
              profileKind: entry.kind,
              profileId: entry.id,
            };
            return (
              <li key={entry.id}>
                <h2>
                  <a
                    href={routePath(route)}
                    onClick={(event) => {
                      if (
                        event.button !== 0 ||
                        event.metaKey ||
                        event.ctrlKey ||
                        event.shiftKey ||
                        event.altKey
                      )
                        return;
                      event.preventDefault();
                      onNavigate(route);
                    }}
                  >
                    {entry.name}
                  </a>
                </h2>
                <p>{entry.description}</p>
                <span>By {entry.ownerDisplay}</span>
                <div className="marketplace-card-meta">
                  <span>Revision {entry.latestRevision}</span>
                  <span>
                    {entry.usesCount} {entry.usesCount === 1 ? 'use' : 'uses'}
                  </span>
                </div>
              </li>
            );
          })}
        </ul>
        {state.loading ? <p role="status">Loading profiles</p> : null}
        {state.error === undefined ? (
          !state.loading && state.items.length === 0 ? (
            <p>No matching profiles</p>
          ) : null
        ) : (
          <div role="alert">
            <p>{state.error}</p>
            <Button
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
            disabled={state.loading}
            onClick={() => {
              setCursor(state.next);
              setState((previous) => ({ ...previous, loading: true }));
            }}
          >
            More profiles
          </Button>
        )}
      </div>
    </section>
  );
}
