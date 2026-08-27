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
import type { SessionSpend } from '@chronoai/cma-sdk';
import type { Launch } from '@chronoai/cma-sdk';
import { listLaunchPage } from '../api/client';
import type { AppRouter } from '../routing/routes';
import { routeClick, routePath } from '../routing/routes';
import {
  deliveryState,
  jsonText,
  reachState,
  shortRevision,
  upstreamRepository,
  workspaceCheckpoint,
} from '../trigger/model';

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
  /** Why the session ended, when it ended badly. */
  readonly ending?: string;
  /** Read this session's reserved-capacity spend from the public REST API. */
  readonly readSpend?: (id: string) => Promise<SessionSpend>;
  /** Navigate to the owning Launch without turning the read into a mutation. */
  readonly navigate?: AppRouter['navigate'];
}

export function SessionDrawer({
  session,
  onClose,
  ending,
  readSpend,
  navigate,
}: SessionDrawerProps) {
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
    let live = true;
    void (async () => {
      let cursor: string | undefined;
      for (let pageNumber = 0; pageNumber < 100; pageNumber += 1) {
        const page = await listLaunchPage({ limit: 100, cursor }, controller.signal);
        const found = page.items.find(
          (launch) =>
            launch.session !== null &&
            jsonText(launch.session.session_id ?? launch.session.id) === session.id,
        );
        if (found !== undefined) {
          if (live) setLaunchRead({ session: session.id, launch: found, failed: false });
          return;
        }
        if (page.next_cursor === null) {
          if (live) setLaunchRead({ session: session.id, launch: null, failed: false });
          return;
        }
        cursor = page.next_cursor;
      }
      throw new Error('Launch inventory exceeded the bounded relationship lookup');
    })().catch(() => {
      if (live && !controller.signal.aborted) {
        setLaunchRead({ session: session.id, launch: null, failed: true });
      }
    });
    return () => {
      live = false;
      controller.abort();
    };
  }, [session.id]);

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
          <TriggerRepository launch={owningLaunch} navigate={navigate} />
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
}: {
  readonly launch: Launch;
  readonly navigate?: AppRouter['navigate'];
}) {
  const upstream = upstreamRepository(launch);
  const fork = launch.fork?.repository;
  const reach = reachState(launch);
  const artifact = launch.diff;
  const canContribute =
    artifact !== null &&
    artifact.total_changed_files > 0 &&
    artifact.total_changed_files <= artifact.changed_file_limit &&
    artifact.expected_branch_head === artifact.observed_branch_head &&
    artifact.validation === 'valid' &&
    artifact.checks.every((check) => check.passed) &&
    reach === 'active';
  const launchRoute = { kind: 'launch', launchId: launch.id } as const;
  const contributionRoute = { kind: 'contribute', launchId: launch.id } as const;

  return (
    <Section title="Trigger repository">
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
        <Fact label="Task delivery">
          {deliveryState(launch)?.replaceAll('_', ' ') ?? 'Pending'}
        </Fact>
        {launch.pull_request === null ? null : (
          <Fact label="Pull request">
            {launch.pull_request.canonical_url === null ? (
              launch.pull_request.state.replaceAll('_', ' ')
            ) : (
              <a href={launch.pull_request.canonical_url} target="_blank" rel="noopener noreferrer">
                #{String(launch.pull_request.number)} {launch.pull_request.title}
              </a>
            )}
          </Fact>
        )}
      </Facts>
      <div className="drawer__repository-actions">
        <a
          className="button button--outline button--sm"
          href={routePath(launchRoute)}
          onClick={
            navigate === undefined ? undefined : (event) => routeClick(event, launchRoute, navigate)
          }
        >
          Launch progress
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
