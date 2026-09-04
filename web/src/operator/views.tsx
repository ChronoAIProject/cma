import { Button } from '../ui/Button';
import { InlineConfirm } from '../ui/InlineConfirm';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { IconTrashOutline16 } from '../ui/icons';
import { Pill } from '../ui/Pill';
import { StateDot } from '../ui/StateDot';
import { fullTimestampOrNone } from '../sessions/sessionTime';
import type { Capability } from '../auth/types';
import {
  actionsIn,
  filtered,
  newestFirst,
  outcomeState,
  readableAction,
  type ActivityFilter,
} from './activity';
import { byRecency, labelFor, nameOf, type PersonLabel } from './directory';
import { controlsFor, environmentResetFor, retirementFor } from './access';
import { PersonName, RecordList, RecordRow } from './RecordList';
import type { ActivityEntry, AdminSession, ChatHost, Person, RuntimeInventory } from './types';

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
                <Pill>
                  {outcomeState(entry.outcome) === undefined ? null : (
                    <StateDot state={outcomeState(entry.outcome)!} />
                  )}
                  {entry.outcome}
                </Pill>
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
      {inventory.fullyObserved && inventory.running !== undefined ? (
        <p className="operator__summary">{String(inventory.running)} running</p>
      ) : (
        /*
         * §14: the running total is withheld while any provider is
         * unobservable — drawn as a quiet notice stating why, not as an
         * absence, because a partial total presented as a total is a lie.
         */
        <div className="notice notice--quiet">
          <span className="notice__detail">
            Running total withheld: a provider could not be asked, so any number shown here would be
            missing part of the answer.
          </span>
        </div>
      )}
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
                        <Pill>
                          <StateDot state="warning" />
                          unaccounted
                        </Pill>
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
                  <Pill>
                    <StateDot state="warning" />
                    unknown
                  </Pill>
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
  readonly confirmingDelete: string | null;
  readonly busy: boolean;
  readonly onConfirm: (id: string | null) => void;
  readonly onConfirmDelete: (id: string | null) => void;
  readonly onStop: (id: string) => void;
  readonly onTerminate: (id: string) => void;
  readonly onDeleteChat: (id: string) => void;
}

/** Every session there is, and the two controls that act on one. */
export function SessionsView({
  sessions,
  capabilities,
  confirming,
  confirmingDelete,
  busy,
  onConfirm,
  onConfirmDelete,
  onStop,
  onTerminate,
  onDeleteChat,
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
                <Pill>{session.status}</Pill>
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
              <>
                {confirming === session.id ? (
                  <>
                    <span className="operator__confirm">End this session?</span>
                    <Button
                      variant="primary"
                      disabled={busy}
                      onClick={() => {
                        onTerminate(session.id);
                      }}
                    >
                      End
                    </Button>
                    <Button
                      onClick={() => {
                        onConfirm(null);
                      }}
                    >
                      Cancel
                    </Button>
                  </>
                ) : (
                  <>
                    <Button
                      disabled={!controls.canStop || busy}
                      title={controls.stopReason}
                      onClick={() => {
                        onStop(session.id);
                      }}
                    >
                      Stop
                    </Button>
                    <Button
                      disabled={!controls.canTerminate || busy}
                      title={controls.terminateReason}
                      onClick={() => {
                        onConfirm(session.id);
                      }}
                    >
                      End
                    </Button>
                    {session.chat ? (
                      <Button
                        size="sm"
                        variant="danger"
                        icon={<IconTrashOutline16 />}
                        disabled={!controls.canDeleteChat || busy}
                        title={controls.deleteChatReason}
                        onClick={() => {
                          onConfirmDelete(session.id);
                        }}
                      >
                        Delete Chat
                      </Button>
                    ) : null}
                  </>
                )}
                {session.chat ? (
                  <RiskConfirmation
                    open={confirmingDelete === session.id}
                    title="Delete this Chat?"
                    description={`Owner ${session.ownerLabel ?? session.owner}. This permanently deletes Chat ${session.id}, its history, checkpoints, and worktree. The owner's other Chats and shared sandbox remain available.`}
                    acknowledgeLabel="I understand this Chat cannot be recovered."
                    confirmLabel="Delete Chat"
                    busy={busy}
                    onCancel={() => {
                      onConfirmDelete(null);
                    }}
                    onConfirm={() => {
                      onDeleteChat(session.id);
                    }}
                  />
                ) : null}
              </>
            }
          />
        );
      })}
    </RecordList>
  );
}

/** Compact age used in host inventory rows. */
export function chatHostAge(createdAt: number, now: number = Date.now()): string {
  if (!Number.isFinite(createdAt)) {
    return 'age unknown';
  }
  const elapsed = Math.max(0, now - createdAt);
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) {
    return 'under 1 min old';
  }
  if (minutes < 60) {
    return `${String(minutes)} min old`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${String(hours)} hr old`;
  }
  return `${String(Math.floor(hours / 24))} d old`;
}

interface ChatHostsViewProps {
  readonly hosts: readonly ChatHost[];
  readonly capabilities: readonly Capability[];
  readonly confirming: string | null;
  readonly confirmingReset: string | null;
  readonly busy: boolean;
  readonly onConfirm: (id: string | null) => void;
  readonly onConfirmReset: (id: string | null) => void;
  readonly onRetire: (id: string) => void;
  readonly onReset: (id: string) => void;
}

/** Deployment-managed shared chat runtimes and their audited retirement control. */
export function ChatHostsView({
  hosts,
  capabilities,
  confirming,
  confirmingReset,
  busy,
  onConfirm,
  onConfirmReset,
  onRetire,
  onReset,
}: ChatHostsViewProps) {
  return (
    <RecordList
      label="Every chat host"
      empty="No shared chat host has been created yet."
      count={hosts.length}
    >
      {hosts.map((host) => {
        const retirement = retirementFor(host, capabilities);
        const reset = environmentResetFor(capabilities);
        return (
          <RecordRow
            key={host.id}
            at={chatHostAge(host.createdAt)}
            primary={
              <>
                <PersonName label={labelFor(host.owner)} />
                <Pill>{host.status}</Pill>
              </>
            }
            meta={
              <>
                <span className="record__identifier">{host.id}</span>
                <span>
                  {String(host.threadCount)} {host.threadCount === 1 ? 'thread' : 'threads'}
                </span>
                {host.runtimeVersion === undefined ? null : (
                  <span>runtime {host.runtimeVersion}</span>
                )}
                {host.lastFailure === undefined ? null : (
                  <>
                    <span className="chat-host__failure-summary">
                      failure {host.lastFailure.phase}
                      {host.lastFailure.commandIndex === undefined
                        ? ''
                        : `, command index ${String(host.lastFailure.commandIndex)}`}
                    </span>
                    {host.lastFailure.outputExcerpt === undefined ? null : (
                      <pre
                        className="chat-host__failure-output"
                        aria-label={`Setup failure output for ${host.id}`}
                      >
                        {host.lastFailure.outputExcerpt === ''
                          ? 'No output captured.'
                          : host.lastFailure.outputExcerpt}
                      </pre>
                    )}
                  </>
                )}
              </>
            }
            actions={
              <>
                {confirming === host.id ? (
                  <InlineConfirm
                    question="Retire this chat host?"
                    confirmLabel="Retire"
                    busy={busy}
                    cancelDisabled={busy}
                    onCancel={() => {
                      onConfirm(null);
                    }}
                    onConfirm={() => {
                      onRetire(host.id);
                    }}
                  />
                ) : (
                  <>
                    <Button
                      size="sm"
                      variant="danger"
                      disabled={!retirement.canRetire || busy}
                      title={retirement.reason}
                      onClick={() => {
                        onConfirm(host.id);
                      }}
                    >
                      Retire
                    </Button>
                    <Button
                      size="sm"
                      variant="danger"
                      icon={<IconTrashOutline16 />}
                      disabled={!reset.canReset || busy}
                      title={reset.reason}
                      onClick={() => {
                        onConfirmReset(host.id);
                      }}
                    >
                      Reset environment
                    </Button>
                    {retirement.reason === undefined ? null : (
                      <span className="control-reason">{retirement.reason}</span>
                    )}
                  </>
                )}
                <RiskConfirmation
                  open={confirmingReset === host.id}
                  title="Reset this Chat environment?"
                  description={`Owner ${host.owner} has ${String(host.threadCount)} ${host.threadCount === 1 ? 'Chat' : 'Chats'} on this host. This permanently deletes every Chat history, checkpoint, and worktree, then terminates the shared sandbox.`}
                  acknowledgeLabel="I understand this owner's complete Chat environment will be deleted."
                  confirmLabel="Reset environment"
                  busy={busy}
                  onCancel={() => {
                    onConfirmReset(null);
                  }}
                  onConfirm={() => {
                    onReset(host.id);
                  }}
                />
              </>
            }
          />
        );
      })}
    </RecordList>
  );
}
