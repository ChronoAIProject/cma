import { useCallback, useEffect, useState } from 'react';
import type { Capability } from '../auth/types';
import type { ActivityFilter } from './activity';
import { namesFrom } from './directory';
import { viewsFor, type OperatorViewId } from './access';
import { ActivityView, NamespaceView, PeopleView, SessionsView } from './views';
import { PlatformSettingsView } from './PlatformSettingsView';
import type { ActivityEntry, AdminSession, OperatorStore, Person, RuntimeInventory } from './types';
import './OperatorPage.css';

/**
 * Running the service — docs/Web_Style.md §14.
 *
 * Five views nobody but an operator can use: who is using it, what it has been
 * asked to do, what is running, acting on a session, and deployment policy.
 *
 * **A page, not a dialog.** It is the one surface that shows many rows at
 * once, and a 560px dialog was why its rows truncated. As a page its five
 * views have somewhere to live without stacking chrome inside a modal: they
 * are a tablist in the page's second band, beneath the head band every column
 * shares.
 *
 * It is §1's capability-gated view, not a route. There is no router and no URL:
 * the account-menu action chooses it, and the choice does not
 * survive a reload, because a view is where somebody is rather than something
 * they configured.
 *
 * **This protects nothing.** Which views are offered is decided from the
 * capabilities the control plane reported, and every read and control behind
 * them is refused server-side on its own capability. A hand-written request
 * gets the same refusal. Both halves are required: a control nobody can use
 * should not be drawn, and a drawn control is not what makes it safe.
 */

interface OperatorPageProps {
  readonly store: OperatorStore;
  readonly capabilities: readonly Capability[];
}

/** What one view has loaded, or why it could not. */
type Loaded<T> = { readonly kind: 'loading' } | { readonly kind: 'ready'; readonly value: T };

export function OperatorPage({ store, capabilities }: OperatorPageProps) {
  const views = viewsFor(capabilities);
  const [active, setActive] = useState<OperatorViewId | undefined>(() => views[0]?.id);
  // Once opened, keep the settings editor mounted so a tab switch cannot erase
  // an unsaved draft. It still makes no request before its view is first used.
  const [settingsOpened, setSettingsOpened] = useState(() => views[0]?.id === 'settings');
  const [error, setError] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [filter, setFilter] = useState<ActivityFilter>({});

  /*
   * A caller who may not read the directory has nothing to load rather than
   * something to wait for, so that is the state they start in. Naming people is
   * its own grant: the activity view still draws, with identifiers where names
   * would be, because a view refused for want of a second capability is a view
   * drawn empty by entitlement.
   */
  const [people, setPeople] = useState<Loaded<readonly Person[]>>(() =>
    capabilities.includes('read_user_directory')
      ? { kind: 'loading' }
      : { kind: 'ready', value: [] },
  );
  const [activity, setActivity] = useState<Loaded<readonly ActivityEntry[]>>({ kind: 'loading' });
  const [inventory, setInventory] = useState<Loaded<RuntimeInventory>>({ kind: 'loading' });
  const [sessions, setSessions] = useState<Loaded<readonly AdminSession[]>>({ kind: 'loading' });

  /**
   * Report a failure in the operator's own terms.
   *
   * Never swallowed and never rendered as an empty list: "there is nothing" and
   * "we could not ask" are the two answers this panel exists to keep apart.
   */
  const failed = useCallback((cause: unknown) => {
    setError(cause instanceof Error ? cause.message : String(cause));
  }, []);

  // Each view loads only what it draws, and only once it is opened: a caller
  // holding one capability must not have a request made on their behalf for a
  // surface they are not entitled to, which would be refused and reported as
  // an error they can do nothing about.
  useEffect(() => {
    if ((active !== 'people' && active !== 'activity') || people.kind === 'ready') {
      return;
    }
    store
      .people()
      .then((value) => {
        setPeople({ kind: 'ready', value });
      })
      .catch(failed);
  }, [active, failed, people.kind, store]);

  useEffect(() => {
    if (active !== 'activity') {
      return;
    }
    store
      .activity({ actor: filter.actor })
      .then((value) => {
        setActivity({ kind: 'ready', value });
      })
      .catch(failed);
    // Narrowing by person is asked of the control plane, so a change of actor
    // is a fresh read rather than a filter over the page already in hand.
  }, [active, failed, filter.actor, store]);

  /**
   * Narrow the record.
   *
   * A change of person puts the view back to waiting, because the answer is
   * coming from the control plane: leaving the previous person's entries on
   * screen under their successor's name is the one reading of this list nobody
   * should be offered. Done here rather than in the effect that reads, since an
   * event is where a reset belongs.
   */
  const narrow = useCallback(
    (next: ActivityFilter) => {
      if (next.actor !== filter.actor) {
        setActivity({ kind: 'loading' });
      }
      setFilter(next);
    },
    [filter.actor],
  );

  useEffect(() => {
    if (active !== 'namespace' || inventory.kind === 'ready') {
      return;
    }
    store
      .runtimes()
      .then((value) => {
        setInventory({ kind: 'ready', value });
      })
      .catch(failed);
  }, [active, failed, inventory.kind, store]);

  const reloadSessions = useCallback(() => {
    store
      .sessions()
      .then((value) => {
        setSessions({ kind: 'ready', value });
      })
      .catch(failed);
  }, [failed, store]);

  useEffect(() => {
    if (active !== 'sessions' || sessions.kind === 'ready') {
      return;
    }
    reloadSessions();
  }, [active, reloadSessions, sessions.kind]);

  /** Run a control, then reread rather than guessing at the new state. */
  const act = useCallback(
    (run: () => Promise<void>) => {
      setBusy(true);
      setError(undefined);
      run()
        .then(() => {
          setConfirming(null);
          reloadSessions();
        })
        .catch(failed)
        .finally(() => {
          setBusy(false);
        });
    },
    [failed, reloadSessions],
  );

  return (
    <section className="operator" aria-label="Operations">
      {/*
       * The centre column's header, in the same idiom as a session's: a crumb
       * saying which of the three views is showing. §9 gave every column its
       * own proportions, so this lines up with the session header it replaces
       * rather than with a band that no longer exists.
       */}
      <div className="sessionheader operator__head">
        <span className="sessionheader__crumb">Operations</span>
      </div>

      <div className="operator__intro">
        <h1 className="operator__title">Operations</h1>
        <p className="operator__description">
          Service activity, running work, session controls, and deployment policy.
        </p>
      </div>

      {/* The five views, as an underlined tablist — §19. */}
      <div className="operator__tabs" role="tablist" aria-label="Operator views">
        {views.map((view) => (
          <button
            key={view.id}
            type="button"
            role="tab"
            id={`operator-tab-${view.id}`}
            aria-selected={active === view.id}
            aria-controls={`operator-view-${view.id}`}
            className={`operator__tab${active === view.id ? ' operator__tab--on' : ''}`}
            onClick={() => {
              setActive(view.id);
              if (view.id === 'settings') {
                setSettingsOpened(true);
              }
              setError(undefined);
            }}
          >
            {view.label}
          </button>
        ))}
      </div>

      {error === undefined ? null : (
        <p className="operator__error" role="alert">
          {error}
        </p>
      )}

      <div
        className="operator__view"
        role="tabpanel"
        id={active === undefined ? undefined : `operator-view-${active}`}
        aria-labelledby={active === undefined ? undefined : `operator-tab-${active}`}
      >
        {active === 'people' ? (
          people.kind === 'loading' ? (
            <Waiting />
          ) : (
            <PeopleView people={people.value} />
          )
        ) : null}

        {active === 'activity' ? (
          activity.kind === 'loading' || people.kind === 'loading' ? (
            <Waiting />
          ) : (
            <ActivityView
              entries={activity.value}
              names={namesFrom(people.value)}
              people={people.value}
              filter={filter}
              onFilter={narrow}
            />
          )
        ) : null}

        {active === 'namespace' ? (
          inventory.kind === 'loading' ? (
            <Waiting />
          ) : (
            <NamespaceView inventory={inventory.value} />
          )
        ) : null}

        {active === 'sessions' ? (
          sessions.kind === 'loading' ? (
            <Waiting />
          ) : (
            <SessionsView
              sessions={sessions.value}
              capabilities={capabilities}
              confirming={confirming}
              busy={busy}
              onConfirm={setConfirming}
              onStop={(id) => {
                act(() => store.stopSession(id));
              }}
              onTerminate={(id) => {
                act(() => store.terminateSession(id));
              }}
            />
          )
        ) : null}

        {settingsOpened ? (
          <div hidden={active !== 'settings'}>
            <PlatformSettingsView store={store} />
          </div>
        ) : null}
      </div>
    </section>
  );
}

/** §5 forbids a spinner, and this claims no knowledge of progress. */
function Waiting() {
  return <p className="operator__waiting">Reading…</p>;
}
