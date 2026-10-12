import { useEffect, useState } from 'react';
import type { EventWorkerProfileRecord, EventWorkerProfiles } from '@chronoai/cma-sdk';
import type { AppRoute } from '../../routing/routes';
import { routePath } from '../../routing/routes';
import { Button } from '../../ui/Button';
import '../ProfileCard.css';
import { problem } from '../../bots/model';

export function EventWorkersHome({
  client,
  onNavigate,
  canManage,
}: {
  readonly client: Pick<EventWorkerProfiles, 'list'>;
  readonly onNavigate: (route: AppRoute) => void;
  readonly canManage: boolean;
}) {
  const [rows, setRows] = useState<readonly EventWorkerProfileRecord[]>([]);
  const [cursor, setCursor] = useState<string>();
  const [next, setNext] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [reload, setReload] = useState(0);
  useEffect(() => {
    document.title = 'Event Worker Profiles · CMA';
    return () => {
      document.title = 'CMA';
    };
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    if (!canManage) return;
    void client
      .list({ after: cursor, signal: controller.signal })
      .then((page) => {
        if (controller.signal.aborted) return;
        if (page.next_after === cursor && cursor !== undefined)
          throw new Error('Profile page did not advance.');
        setRows((current) =>
          cursor === undefined
            ? page.items
            : [
                ...current,
                ...page.items.filter((row) => !current.some((item) => item.id === row.id)),
              ],
        );
        setNext(page.next_after ?? undefined);
        setLoading(false);
        setError(undefined);
      })
      .catch((failure: unknown) => {
        if (!controller.signal.aborted) {
          setError(problem(failure));
          setLoading(false);
        }
      });
    return () => controller.abort();
  }, [client, cursor, reload, canManage]);
  return (
    <section className="profiles-home">
      <header className="profiles-home__header">
        <h1>Event Worker Profiles</h1>
        {canManage ? (
          <Button
            variant="primary"
            onClick={() => onNavigate({ kind: 'event_worker_profile', profileId: 'new' })}
          >
            New worker profile
          </Button>
        ) : null}
      </header>
      <p>
        A complete, versioned configuration for a Bot’s model, permissions, context and environment.
      </p>
      {!canManage ? (
        <p>You do not have permission to manage profiles.</p>
      ) : (
        <>
          <ul className="profile-card-grid">
            {rows.map((row) => {
              const route: AppRoute = { kind: 'event_worker_profile', profileId: row.id };
              return (
                <li key={row.id}>
                  <article className="profile-card">
                    <div className="profile-card__kind">Event worker</div>
                    <h2>
                      <a
                        href={routePath(route)}
                        onClick={(event) => {
                          if (
                            event.button === 0 &&
                            !event.metaKey &&
                            !event.ctrlKey &&
                            !event.shiftKey &&
                            !event.altKey
                          ) {
                            event.preventDefault();
                            onNavigate(route);
                          }
                        }}
                      >
                        {row.source.name}
                      </a>
                    </h2>
                    <div className="profile-card__badges">Revision {row.revision}</div>
                    <p className="profile-card__description">{row.source.description}</p>
                    <footer className="profile-card__footer">
                      <span>{row.profile.model.model}</span>
                      <time dateTime={row.updated_at}>
                        Updated {new Date(row.updated_at).toLocaleDateString()}
                      </time>
                    </footer>
                  </article>
                </li>
              );
            })}
          </ul>
          {loading ? <p role="status">Loading worker profiles</p> : null}
          {error === undefined ? (
            !loading && rows.length === 0 ? (
              <p>No Event Worker Profiles yet. Create one from your published CMA profiles.</p>
            ) : null
          ) : (
            <div role="alert">
              <p>{error}</p>
              <Button
                onClick={() => {
                  setLoading(true);
                  setReload((n) => n + 1);
                }}
              >
                Try again
              </Button>
            </div>
          )}
          {next === undefined ? null : (
            <Button
              disabled={loading}
              onClick={() => {
                setLoading(true);
                setCursor(next);
              }}
            >
              More worker profiles
            </Button>
          )}
        </>
      )}
    </section>
  );
}
