import { useEffect, useState } from 'react';
import type { CredentialMode, PermissionMode, Session } from './types';
import { abilitiesFor } from './availability';
import {
  credentialWords,
  effortWords,
  labelFor,
  permissionWords,
  stateDotFor,
  toneFor,
} from './types';
import { fullTimestamp, fullTimestampOrNone } from './sessionTime';
import { StateDot } from '../ui/StateDot';
import { CreatorFace, Fact, Facts, ReachList, Section, SkillList, Unknown } from './SessionFacts';
import './SessionDrawer.css';
import { IconCloseOutline16 } from '../ui/icons';
import type { Launch, SessionResourceStatus, SessionSpend } from '@chronoai/cma-sdk';
import { listLaunchPage, readLaunch } from '../api/client';
import type { AppRouter } from '../routing/routes';
import { routeClick, routePath } from '../routing/routes';
import { Resources } from '../chat/Resources';
import {
  deliveryState,
  mergeabilityLabel,
  reachState,
  shortRevision,
  stateLabel,
  upstreamRepository,
  workspaceCheckpoint,
} from '../trigger/model';

const TRIGGER_LAUNCH_REFRESH_INTERVAL_MS = 15_000;
const CHAT_RESOURCE_REFRESH_INTERVAL_MS = 2_000;

function pullRequestChecksLabel(value: NonNullable<Launch['pull_request']>['checks']): string {
  return value === 'awaiting_maintainer_approval'
    ? 'Awaiting maintainer approval'
    : stateLabel(value);
}

function pullRequestFailureNote(
  failure: NonNullable<Launch['pull_request']>['status_failure'],
): string | undefined {
  if (failure === null) return undefined;
  const retry = failure.retry_at === undefined ? '' : ` Retry after ${failure.retry_at}.`;
  return `${failure.detail} ${failure.remediation}${retry}`;
}

/**
 * What is known about the open session — `docs/Web_Style.md` §17.
 *
 * A right drawer, 360px, closed by default and opened from the header's
 * Details key. It is reference material: read when somebody wants to know
 * which agent is answering, what it was given, or why a session stopped.
 *
 * The previous revision drew every fact as one flat list of eighteen rows,
 * which is what "just a lot of text" describes — nothing said which facts
 * belonged together, so finding one meant reading all of them. This revision
 * groups them into five bands under their own titles, and each band answers a
 * different question: what state is it in, what is answering, what was it
 * given, what can it touch, what is it running on.
 *
 * Below 75rem it overlays the surface rather than narrowing it (§16).
 */

interface SessionDrawerProps {
  readonly session: Session;
  readonly onClose: () => void;
  /** Whether this always-mounted drawer is actually visible. */
  readonly open?: boolean;
  /** Why the session ended, when it ended badly. */
  readonly ending?: string;
  /** Read this session's reserved-capacity spend from the public REST API. */
  readonly readSpend?: (id: string) => Promise<SessionSpend>;
  /** Read the latest CPU and memory status for an active Chat host. */
  readonly readResources?: (id: string, signal?: AbortSignal) => Promise<SessionResourceStatus>;
  /** Navigate to the owning Launch without turning the read into a mutation. */
  readonly navigate?: AppRouter['navigate'];
}

export function SessionDrawer(props: SessionDrawerProps) {
  return props.session.chat === undefined ? (
    <AgentSessionDrawer {...props} />
  ) : (
    <ChatSessionDrawer
      session={props.session}
      chat={props.session.chat}
      onClose={props.onClose}
      open={props.open ?? true}
      readResources={props.readResources}
    />
  );
}

/** The focused Chat drawer: identity, worktree facts, and live host resources. */
function ChatSessionDrawer({
  session,
  chat,
  onClose,
  open,
  readResources,
}: Pick<SessionDrawerProps, 'session' | 'onClose' | 'open' | 'readResources'> & {
  readonly chat: NonNullable<Session['chat']>;
}) {
  const [copied, setCopied] = useState(false);
  const [resourceRead, setResourceRead] = useState<{
    readonly session: string;
    readonly status?: SessionResourceStatus;
    readonly failed: boolean;
  }>();
  const tone = toneFor(session.lifecycle);
  const dot = stateDotFor(tone);
  const hostActive = session.lifecycle !== 'idle' && session.lifecycle !== 'terminated';
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => {
      setCopied(false);
    }, 2000);
    return () => {
      window.clearTimeout(timer);
    };
  }, [copied]);

  useEffect(() => {
    if (
      open !== true ||
      !hostActive ||
      !session.capabilities.resources ||
      readResources === undefined
    ) {
      return;
    }

    const controller = new AbortController();
    let live = true;
    let timer: number | undefined;
    const readResourceStatus = readResources;

    async function refresh(): Promise<void> {
      try {
        const status = await readResourceStatus(session.id, controller.signal);
        if (live) {
          setResourceRead({ session: session.id, status, failed: false });
        }
      } catch {
        if (live && !controller.signal.aborted) {
          setResourceRead({ session: session.id, failed: true });
        }
      } finally {
        if (live) {
          timer = window.setTimeout(() => {
            void refresh();
          }, CHAT_RESOURCE_REFRESH_INTERVAL_MS);
        }
      }
    }

    void refresh();
    return () => {
      live = false;
      controller.abort();
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [hostActive, open, readResources, session.capabilities.resources, session.id]);

  const currentResourceRead = resourceRead?.session === session.id ? resourceRead : undefined;

  const copyPath = () => {
    void navigator.clipboard
      ?.writeText(chat.worktreePath)
      .then(() => {
        setCopied(true);
      })
      .catch(() => {
        // The complete path remains selectable if clipboard access is refused.
      });
  };

  return (
    <aside className="drawer drawer--chat" aria-label="Chat details">
      <button
        type="button"
        className="drawer__close"
        aria-label="Close chat details"
        onClick={onClose}
      >
        <IconCloseOutline16 size={14} />
      </button>

      <div className="drawer__body">
        <header className="drawer__band" data-tone={tone}>
          <p className="drawer__state">
            {dot === undefined ? null : <StateDot state={dot} placement="drawer__dot" />}
            <span className="drawer__statename">
              {labelFor(session.lifecycle, session.reportedStatus)}
            </span>
          </p>
          <p className="drawer__started">Started {fullTimestamp(session.createdAt)}</p>
          {session.creator === undefined ? null : (
            <p className="drawer__creator">
              <CreatorFace creator={session.creator} />
              <span>{session.creator.displayName ?? 'Not named'}</span>
            </p>
          )}
          <p className="drawer__chattitle">{chat.title ?? chat.worktreeName}</p>
        </header>

        <Section title="Worktree">
          <Facts>
            <Fact label="Name">{chat.worktreeName}</Fact>
            <Fact label="Local path">
              <button
                type="button"
                className="drawer__path"
                aria-label={`Copy local path ${chat.worktreePath}`}
                onClick={copyPath}
              >
                <code className="drawer__mono">{chat.worktreePath}</code>
                <span className="drawer__pathhint">{copied ? 'Copied' : 'Copy'}</span>
              </button>
            </Fact>
          </Facts>
        </Section>

        <Section title="Resources">
          <div className="drawer__resources">
            {!hostActive ? (
              <span className="resources__status resources__status--drawer">
                CPU and memory are available while the chat host is active
              </span>
            ) : currentResourceRead?.failed === true ? (
              <span className="resources__status resources__status--drawer">
                Resource status is temporarily unavailable
              </span>
            ) : (
              <Resources
                granted={session.capabilities.resources}
                unreported={currentResourceRead?.status?.reason === 'runtime_does_not_report'}
                resources={currentResourceRead?.status?.reading}
                drawer
              />
            )}
          </div>
        </Section>
      </div>
    </aside>
  );
}

function AgentSessionDrawer({ session, onClose, ending, readSpend, navigate }: SessionDrawerProps) {
  const can = abilitiesFor(session);
  const [copied, setCopied] = useState(false);
  const [spendRead, setSpendRead] = useState<{
    readonly session: string;
    readonly spend?: SessionSpend;
    readonly failed: boolean;
  }>();
  const [launchRead, setLaunchRead] = useState<{
    readonly session: string;
    readonly launch: Launch | null;
    readonly failed: boolean;
  }>();
  const tone = toneFor(session.lifecycle);
  const dot = stateDotFor(tone);
  const agent = session.agent;

  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = window.setTimeout(() => {
      setCopied(false);
    }, 2000);
    return () => {
      window.clearTimeout(timer);
    };
  }, [copied]);

  useEffect(() => {
    if (readSpend === undefined) {
      return;
    }
    let live = true;
    void readSpend(session.id)
      .then((next) => {
        if (live) {
          setSpendRead({ session: session.id, spend: next, failed: false });
        }
      })
      .catch(() => {
        if (live) {
          setSpendRead({ session: session.id, failed: true });
        }
      });
    return () => {
      live = false;
    };
  }, [readSpend, session.id]);

  useEffect(() => {
    const controller = new AbortController();
    let timer: number | undefined;
    let live = true;

    function scheduleDiscovery(): void {
      timer = window.setTimeout(() => {
        void discover();
      }, TRIGGER_LAUNCH_REFRESH_INTERVAL_MS);
    }

    async function discover(): Promise<void> {
      try {
        const page = await listLaunchPage({ session_id: session.id, limit: 1 }, controller.signal);
        const found = page.items[0];
        if (found !== undefined) {
          if (live) setLaunchRead({ session: session.id, launch: found, failed: false });
          return;
        }
        if (live) {
          setLaunchRead({ session: session.id, launch: null, failed: false });
          scheduleDiscovery();
        }
      } catch {
        if (live && !controller.signal.aborted) {
          setLaunchRead({ session: session.id, launch: null, failed: true });
          scheduleDiscovery();
        }
      }
    }

    void discover();
    return () => {
      live = false;
      controller.abort();
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [session.id]);

  const discoveredLaunchId =
    launchRead?.session === session.id ? (launchRead.launch?.id ?? null) : null;

  useEffect(() => {
    if (discoveredLaunchId === null) return;
    const controller = new AbortController();
    let timer: number | undefined;
    let live = true;

    const schedule = () => {
      timer = window.setTimeout(refresh, TRIGGER_LAUNCH_REFRESH_INTERVAL_MS);
    };

    const refresh = () => {
      void readLaunch(discoveredLaunchId, controller.signal)
        .then((next) => {
          if (live) setLaunchRead({ session: session.id, launch: next, failed: false });
        })
        .catch(() => {
          if (!live || controller.signal.aborted) return;
          setLaunchRead((current) =>
            current?.session === session.id ? { ...current, failed: true } : current,
          );
        })
        .finally(() => {
          if (live) schedule();
        });
    };

    refresh();
    return () => {
      live = false;
      controller.abort();
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [discoveredLaunchId, session.id]);

  const currentSpend = spendRead?.session === session.id ? spendRead.spend : undefined;
  const spendFailed = spendRead?.session === session.id && spendRead.failed;
  const owningLaunch = launchRead?.session === session.id ? launchRead.launch : null;
  const launchLookupFailed = launchRead?.session === session.id && launchRead.failed;

  const copyIdentifier = () => {
    /*
     * Clipboard access needs a secure context and can be refused. The control
     * only claims "Copied" after the write resolved — claiming it before
     * would report an action that did not happen.
     */
    void navigator.clipboard
      ?.writeText(session.id)
      .then(() => {
        setCopied(true);
      })
      .catch(() => {
        // Refused by the browser. The identifier is still selectable text.
      });
  };

  return (
    <aside className="drawer" aria-label="Session details">
      {/*
       * No title. The panel is named to assistive technology by the `aside`'s
       * own label, and a heading reading "Details" over a list of details told
       * a sighted reader what they were already looking at.
       *
       * The close key floats over the corner rather than sitting in a band of
       * its own, for the same reason: a band exists to hold a title.
       */}
      <button
        type="button"
        className="drawer__close"
        aria-label="Close session details"
        onClick={onClose}
      >
        <IconCloseOutline16 size={14} />
      </button>

      <div className="drawer__body">
        {/*
         * The band, carrying the three things somebody checks before reading
         * anything else: what state it is in, when it started, and whose it is.
         *
         * This is the one place in the panel that takes a colour, and it takes
         * the session's own state tone — the vocabulary §12.1 already defines,
         * not a colour chosen for this surface. A finished session draws no dot
         * at all, because every state the dot has asserts something about right
         * now and a finished session asserts none of them.
         */}
        <header className="drawer__band" data-tone={tone}>
          <p className="drawer__state">
            {dot === undefined ? null : <StateDot state={dot} placement="drawer__dot" />}
            <span className="drawer__statename">
              {labelFor(session.lifecycle, session.reportedStatus)}
            </span>
          </p>
          <p className="drawer__started">Started {fullTimestamp(session.createdAt)}</p>
          {/*
           * Omitted entirely when the control plane did not name them, rather
           * than drawn as a row with nothing in it — naming somebody is its
           * own grant, and a caller who does not hold it is told nothing.
           */}
          {session.creator === undefined ? null : (
            <p className="drawer__creator">
              <CreatorFace creator={session.creator} />
              <span>{session.creator.displayName ?? 'Not named'}</span>
            </p>
          )}
        </header>

        {/*
         * What is answering, and on whose terms. Read from the session and
         * never from the profile it came from: a profile can be edited after a
         * session started from it and the session keeps what it was
         * provisioned with, so the profile answers what a *new* session would
         * get and this answers what this one has.
         */}
        <Section title="Agent">
          <Facts>
            <Fact label="Kernel">{session.kernel}</Fact>
            <Fact label="Model">
              {agent?.model ?? <Unknown>The kernel&rsquo;s own default</Unknown>}
            </Fact>
            <Fact label="Reasoning">
              {agent?.reasoningEffort === undefined ? (
                <Unknown>Not reported</Unknown>
              ) : (
                effortWords(agent.reasoningEffort)
              )}
            </Fact>
            <Permission mode={agent?.permission} />
            <Credential mode={agent?.credential} />
            {readSpend === undefined ? null : (
              <CapacitySpend spend={currentSpend} failed={spendFailed} />
            )}
          </Facts>
        </Section>

        {/*
         * What was installed into its sandbox before it started. Somebody
         * asking why a slash command is missing has no other way to find out
         * whether the skill is there.
         */}
        <Section title="Skills" count={agent?.skills?.length}>
          <SkillList
            skills={agent?.skills}
            installing={session.lifecycle === 'provisioning' || session.lifecycle === 'rotating'}
          />
        </Section>

        {/*
         * What it can touch, beside what it can do. §11 and §17: an owner
         * watching an agent work on their behalf has one question the
         * interface did not answer — what does this thing have access to?
         */}
        <Section
          title="Reaches"
          count={session.reach?.length}
          note="NyxID services which this session is scoped to."
        >
          <ReachList services={session.reach} />
        </Section>

        {owningLaunch === null ? (
          launchLookupFailed ? (
            <Section title="Trigger repository">
              <Unknown>Could not be read</Unknown>
            </Section>
          ) : null
        ) : (
          <TriggerRepository launch={owningLaunch} navigate={navigate} stale={launchLookupFailed} />
        )}

        {/*
         * Only worth a band once there is something to say. A session that has
         * never been moved and reports no version would draw two empty rows.
         */}
        {session.runtimeVersion === undefined && !hasMoved(session) ? null : (
          <Section title="Runtime">
            <Facts>
              {session.runtimeVersion === undefined ? null : (
                <Fact label="Version">
                  <code className="drawer__mono">{session.runtimeVersion}</code>
                </Fact>
              )}
              {/*
               * Shown because the alternative is somebody noticing a gap in
               * their terminal and having no way to find out what caused it.
               */}
              {hasMoved(session) ? (
                <Fact
                  label="Moved"
                  note={
                    session.rotatedAt === undefined
                      ? undefined
                      : `Carried forward from ${fullTimestampOrNone(session.rotatedAt)}`
                  }
                >
                  {session.rotations === 1 ? 'Once' : `${String(session.rotations)} times`}
                </Fact>
              ) : null}
            </Facts>
          </Section>
        )}

        <Section title="Identifier">
          {/*
           * Copyable, because it is what support and logs refer to, and a
           * partial identifier is not one anyone can use.
           */}
          <button type="button" className="drawer__id" onClick={copyIdentifier}>
            <span className="drawer__idvalue">{session.id}</span>
            <span className="drawer__idhint">{copied ? 'Copied' : 'Copy'}</span>
          </button>
        </Section>

        {/*
         * The reason the session cannot be typed into, stated wherever it is
         * true rather than only when something has gone wrong. A terminal that
         * ignores keystrokes without saying why is the thing people report as
         * broken.
         */}
        {can.reason === undefined ? null : <p className="drawer__reason">{can.reason}</p>}

        {ending !== undefined && ending !== '' ? <p className="drawer__ending">{ending}</p> : null}
      </div>

      {/*
       * No foot. Every lifecycle act is one key in the session header now, and
       * splitting them across two surfaces is how somebody comes looking for
       * an action where it used to be.
       *
       * This panel is reference material, and reference material that also
       * acts is a panel people are afraid to open.
       */}
    </aside>
  );
}

function TriggerRepository({
  launch,
  navigate,
  stale,
}: {
  readonly launch: Launch;
  readonly navigate?: AppRouter['navigate'];
  readonly stale: boolean;
}) {
  const upstream = upstreamRepository(launch);
  const fork = launch.fork?.repository;
  const reach = reachState(launch);
  const delivery = deliveryState(launch);
  const artifact = launch.diff;
  const promptRecoveryRequired = delivery === 'recovery_required';
  const pullRequestBlocksSameLaunchContribution =
    launch.pull_request?.state === 'merged' ||
    launch.pull_request?.state === 'closed_unmerged' ||
    launch.pull_request?.head_matches_launch === false;
  const canContribute =
    !stale &&
    artifact !== null &&
    artifact.total_changed_files > 0 &&
    artifact.total_changed_files <= artifact.changed_file_limit &&
    artifact.expected_branch_head === artifact.observed_branch_head &&
    artifact.validation === 'valid' &&
    artifact.checks.every((check) => check.passed) &&
    reach === 'active' &&
    !pullRequestBlocksSameLaunchContribution;
  const launchRoute = { kind: 'launch', launchId: launch.id } as const;
  const contributionRoute = { kind: 'contribute', launchId: launch.id } as const;

  return (
    <Section
      title="Trigger repository"
      note={stale ? 'The latest Launch refresh failed; provider status may be stale.' : undefined}
    >
      <Facts>
        <Fact label="Upstream">
          {upstream.canonical_owner}/{upstream.canonical_name}
        </Fact>
        <Fact label="Fork">
          {fork === undefined ? 'Pending' : `${fork.canonical_owner}/${fork.canonical_name}`}
        </Fact>
        <Fact label="Branch">{launch.branch?.name ?? 'Pending'}</Fact>
        <Fact label="Base">
          <code className="drawer__mono">{shortRevision(launch.source_sha)}</code>
        </Fact>
        <Fact label="Head">
          {launch.branch === null ? (
            'Pending'
          ) : (
            <code className="drawer__mono">{shortRevision(launch.branch.current_head)}</code>
          )}
        </Fact>
        <Fact label="Checkpoint">{workspaceCheckpoint(launch) ?? 'Not yet'}</Fact>
        <Fact label="GitHub reach">{reach?.replaceAll('_', ' ') ?? 'Pending'}</Fact>
        <Fact label="Task delivery">{delivery?.replaceAll('_', ' ') ?? 'Pending'}</Fact>
        {launch.pull_request === null ? null : (
          <>
            <Fact label="Pull request">
              {launch.pull_request.canonical_url === null ? (
                stateLabel(launch.pull_request.state)
              ) : (
                <a
                  href={launch.pull_request.canonical_url}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  {launch.pull_request.number === null
                    ? launch.pull_request.title
                    : `#${String(launch.pull_request.number)} ${launch.pull_request.title}`}
                </a>
              )}
            </Fact>
            <Fact label="PR state">{stateLabel(launch.pull_request.state)}</Fact>
            <Fact label="PR base">
              <code className="drawer__mono">{launch.pull_request.base_ref}</code>
            </Fact>
            <Fact label="PR head">
              <code className="drawer__mono">{launch.pull_request.head_ref}</code>
              {' @ '}
              <code className="drawer__mono">
                {shortRevision(
                  launch.pull_request.head_revision ?? launch.pull_request.expected_head_revision,
                )}
              </code>
            </Fact>
            <Fact label="Mergeability">{mergeabilityLabel(launch.pull_request.mergeability)}</Fact>
            <Fact label="Checks">{pullRequestChecksLabel(launch.pull_request.checks)}</Fact>
            <Fact label="CMA head binding">
              {launch.pull_request.head_matches_launch === null
                ? 'Not observed'
                : launch.pull_request.head_matches_launch
                  ? 'Matches Launch'
                  : 'Changed outside CMA'}
            </Fact>
            <Fact
              label="Status sync"
              note={pullRequestFailureNote(launch.pull_request.status_failure)}
            >
              {launch.pull_request.status_failure === null ? 'Current' : 'Paused by CMA'}
            </Fact>
            {launch.pull_request.provider_request_id === null ? null : (
              <Fact label="Provider request">
                <code className="drawer__mono">{launch.pull_request.provider_request_id}</code>
              </Fact>
            )}
          </>
        )}
      </Facts>
      {promptRecoveryRequired ? (
        <p className="drawer__reason" role="status">
          CMA cannot determine whether the previous initial task started. Authorizing another
          dispatch may execute the task twice.
        </p>
      ) : null}
      <div className="drawer__repository-actions">
        <a
          className="button button--outline button--sm"
          href={routePath(launchRoute)}
          onClick={
            navigate === undefined ? undefined : (event) => routeClick(event, launchRoute, navigate)
          }
        >
          {promptRecoveryRequired ? 'Review initial-task recovery' : 'Launch progress'}
        </a>
        {canContribute ? (
          <a
            className="button button--primary button--sm"
            href={routePath(contributionRoute)}
            onClick={
              navigate === undefined
                ? undefined
                : (event) => routeClick(event, contributionRoute, navigate)
            }
          >
            Contribute
          </a>
        ) : null}
      </div>
    </Section>
  );
}

/** Whether this session has ever been carried onto another runtime. */
function hasMoved(session: Session): boolean {
  return session.rotations !== undefined && session.rotations > 0;
}

/**
 * How much the agent may do before it asks.
 *
 * The one row in the panel that can take an alarming colour, and it takes it
 * from the mode rather than from this surface's own judgement — the core
 * exposes which mode is exceptional precisely so two surfaces cannot disagree
 * about it.
 */
function Permission({ mode }: { readonly mode?: PermissionMode }) {
  if (mode === undefined) {
    return (
      <Fact label="Permission">
        <Unknown>Not reported</Unknown>
      </Fact>
    );
  }
  const words = permissionWords(mode);
  return (
    <Fact label="Permission" tone={words.exceptional ? 'fail' : undefined} note={words.note}>
      {words.name}
    </Fact>
  );
}

/**
 * Whose account the model calls are spent against.
 *
 * Never a credential and never an endpoint — which *kind* pays is a fact
 * somebody needs, and the value behind it is not.
 */
function Credential({ mode }: { readonly mode?: CredentialMode }) {
  if (mode === undefined) {
    return (
      <Fact label="Billed to">
        <Unknown>Not reported</Unknown>
      </Fact>
    );
  }
  const words = credentialWords(mode);
  return (
    <Fact label="Billed to" note={words.note}>
      {words.name}
    </Fact>
  );
}

/** The deployment's capacity charge, kept distinct from model-vendor billing. */
function CapacitySpend({
  spend,
  failed,
}: {
  readonly spend?: SessionSpend;
  readonly failed: boolean;
}) {
  if (failed) {
    return (
      <Fact label="Capacity spend">
        <Unknown>Could not be read</Unknown>
      </Fact>
    );
  }
  if (spend === undefined) {
    return (
      <Fact label="Capacity spend">
        <Unknown>Reading</Unknown>
      </Fact>
    );
  }
  if (!spend.priced) {
    return (
      <Fact label="Capacity spend">
        <Unknown>Not priced</Unknown>
      </Fact>
    );
  }

  const coverage =
    spend.unboundedIntervals > 0 ? ' This figure covers less than the session’s whole life.' : '';
  const note = spend.accruing
    ? `Accruing while this session is running.${coverage}`
    : `Final reserved-capacity charge.${coverage}`;
  return (
    <Fact label="Capacity spend" note={note}>
      {spend.display}
    </Fact>
  );
}
