import { Chip } from '../ui/Chip';
import { fullTimestampOrNone } from '../sessions/sessionTime';
import type { Capability } from '../auth/types';
import {
  actionsIn,
  filtered,
  newestFirst,
  outcomeTone,
  readableAction,
  type ActivityFilter,
} from './activity';
import { byRecency, labelFor, nameOf, type PersonLabel } from './directory';
import { controlsFor } from './access';
import { PersonName, RecordList, RecordRow } from './RecordList';
import type { ActivityEntry, AdminSession, Person, RuntimeInventory } from './types';

/**
 * The four views of the operator panel.
 *
 * Rendering only. Every ordering and naming rule they obey lives in the modules
 * beside this one, so the same rule cannot be stated differently in two views —
 * and so it stays testable without a DOM.
 */

/** People, most recently seen first. */
export function PeopleView({ people }: { readonly people: readonly Person[] }) {
  return (
    <RecordList
      label="People who have used this service"
      empty="Nobody has signed in yet."
      count={people.length}
    >
      {byRecency(people).map((person) => (
        <RecordRow
          key={person.subject}
          at={fullTimestampOrNone(person.lastSeen)}
          primary={<PersonName label={labelFor(person.subject, undefined, nameOf(person))} />}
          meta={
            <>
              {person.email !== undefined && person.email !== nameOf(person) ? (
                <span>{person.email}</span>
              ) : null}
              <span>first seen {fullTimestampOrNone(person.firstSeen)}</span>
            </>
          }
        />
      ))}
    </RecordList>
  );
}

interface ActivityViewProps {
  readonly entries: readonly ActivityEntry[];
  readonly names: ReadonlyMap<string, string>;
  /** Every subject offered in the person filter, whether or not they acted. */
  readonly people: readonly Person[];
  readonly filter: ActivityFilter;
  readonly onFilter: (filter: ActivityFilter) => void;
}

/** What the service was asked to do, newest first and narrowable. */
export function ActivityView({ entries, names, people, filter, onFilter }: ActivityViewProps) {
  const shown = newestFirst(filtered(entries, filter));
  return (
    <>
      <div className="operator__filters">
        <label className="operator__filter">
          <span className="operator__filter-label">Person</span>
          <select
            value={filter.actor ?? ''}
            onChange={(event) => {
              // Undefined rather than the empty string: the port narrows by
              // asking the control plane, and an empty actor is not a filter.
              const actor = event.target.value === '' ? undefined : event.target.value;
              onFilter({ ...filter, actor });
            }}
          >
            <option value="">Everyone</option>
            {byRecency(people).map((person) => (
              <option key={person.subject} value={person.subject}>
                {nameOf(person) ?? person.subject}
              </option>
            ))}
          </select>
        </label>
        <label className="operator__filter">
          <span className="operator__filter-label">Action</span>
          <select
            value={filter.action ?? ''}
            onChange={(event) => {
              const action = event.target.value === '' ? undefined : event.target.value;
              onFilter({ ...filter, action });
            }}
          >
            <option value="">Everything</option>
            {actionsIn(entries).map((action) => (
              <option key={action} value={action}>
                {readableAction(action)}
              </option>
            ))}
          </select>
        </label>
      </div>

      <RecordList
        label="What this service was asked to do"
        empty="Nothing has been recorded yet."
        count={shown.length}
      >
        {shown.map((entry) => (
          <RecordRow
            key={entry.id}
            at={fullTimestampOrNone(entry.at)}
            primary={
              <>
                {readableAction(entry.action)}
                <Chip tone={outcomeTone(entry.outcome)} inline>
                  {entry.outcome}
                </Chip>
              </>
            }
            meta={
              <>
                <PersonName label={labelFor(entry.actor, names)} />
                {entry.target === undefined ? null : (
                  <span className="record__identifier">{entry.target.id}</span>
                )}
                {entry.detail.map((pair) => (
                  <span key={pair.key}>
                    {pair.key}: {pair.value}
                  </span>
                ))}
              </>
            }
          />
        ))}
      </RecordList>
    </>
  );
}

/**
 * What every provider is running.
 *
 * A provider nobody could ask renders as unknown carrying its reason, and never
 * as one running nothing. The running total is withheld entirely while any
 * provider is unobservable, because a partial total presented as a total is the
 * same lie as an empty list.
 */
export function NamespaceView({ inventory }: { readonly inventory: RuntimeInventory }) {
  return (
    <>
      <p className="operator__summary">
        {inventory.fullyObserved && inventory.running !== undefined
          ? `${String(inventory.running)} running`
          : 'Running total unavailable: a provider could not be asked.'}
      </p>
      <RecordList
        label="Runtimes in service"
        empty="No provider is configured."
        count={inventory.providers.length}
      >
        {inventory.providers.map((provider) =>
          provider.observable ? (
            provider.runtimes.length === 0 ? (
              <RecordRow
                key={provider.kernel}
                primary={provider.kernel}
                meta={<span>running nothing</span>}
              />
            ) : (
              provider.runtimes.map((runtime) => (
                <RecordRow
                  key={`${provider.kernel}:${runtime.runtime}`}
                  primary={
                    <>
                      {provider.kernel}
                      {runtime.unaccounted ? (
                        <Chip tone="warn" inline>
                          unaccounted
                        </Chip>
                      ) : null}
                    </>
                  }
                  meta={
                    <>
                      <span className="record__identifier">{runtime.runtime}</span>
                      {runtime.session === undefined ? null : (
                        <span className="record__identifier">{runtime.session}</span>
                      )}
                      {runtime.status === undefined ? null : <span>{runtime.status}</span>}
                    </>
                  }
                />
              ))
            )
          ) : (
            <RecordRow
              key={provider.kernel}
              primary={
                <>
                  {provider.kernel}
                  <Chip tone="warn" inline>
                    unknown
                  </Chip>
                </>
              }
              meta={<span>{provider.reason ?? 'This provider could not be asked.'}</span>}
            />
          ),
        )}
      </RecordList>
    </>
  );
}

interface SessionsViewProps {
  readonly sessions: readonly AdminSession[];
  readonly capabilities: readonly Capability[];
  /** Which session is awaiting an inline confirmation to end. */
  readonly confirming: string | null;
  readonly busy: boolean;
  readonly onConfirm: (id: string | null) => void;
  readonly onStop: (id: string) => void;
  readonly onTerminate: (id: string) => void;
}

/** Every session there is, and the two controls that act on one. */
export function SessionsView({
  sessions,
  capabilities,
  confirming,
  busy,
  onConfirm,
  onStop,
  onTerminate,
}: SessionsViewProps) {
  return (
    <RecordList
      label="Every session"
      empty="Nothing is running, and nothing has been."
      count={sessions.length}
    >
      {sessions.map((session) => {
        const controls = controlsFor(session, capabilities);
        const owner: PersonLabel = labelFor(session.owner, undefined, session.ownerLabel);
        return (
          <RecordRow
            key={session.id}
            at={fullTimestampOrNone(session.createdAt)}
            primary={
              <>
                <PersonName label={owner} />
                <Chip inline>{session.status}</Chip>
              </>
            }
            meta={
              <>
                <span className="record__identifier">{session.id}</span>
                <span>{session.kernel}</span>
                {session.runtimeVersion === undefined ? null : (
                  <span>runtime {session.runtimeVersion}</span>
                )}
              </>
            }
            actions={
              confirming === session.id ? (
                <>
                  <span className="operator__confirm">End this session?</span>
                  <button
                    type="button"
                    className="button button--danger"
                    disabled={busy}
                    onClick={() => {
                      onTerminate(session.id);
                    }}
                  >
                    End
                  </button>
                  <button
                    type="button"
                    className="button"
                    onClick={() => {
                      onConfirm(null);
                    }}
                  >
                    Cancel
                  </button>
                </>
              ) : (
                <>
                  <button
                    type="button"
                    className="button"
                    disabled={!controls.canStop || busy}
                    title={controls.stopReason}
                    onClick={() => {
                      onStop(session.id);
                    }}
                  >
                    Stop
                  </button>
                  <button
                    type="button"
                    className="button"
                    disabled={!controls.canTerminate || busy}
                    title={controls.terminateReason}
                    onClick={() => {
                      onConfirm(session.id);
                    }}
                  >
                    End
                  </button>
                </>
              )
            }
          />
        );
      })}
    </RecordList>
  );
}
