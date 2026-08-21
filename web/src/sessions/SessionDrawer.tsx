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
}

export function SessionDrawer({ session, onClose, ending }: SessionDrawerProps) {
  const can = abilitiesFor(session);
  const [copied, setCopied] = useState(false);
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
            <span className="drawer__statename">{labelFor(session.lifecycle)}</span>
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
          </Facts>
        </Section>

        {/*
         * What was installed into its sandbox before it started. Somebody
         * asking why a slash command is missing has no other way to find out
         * whether the skill is there.
         */}
        <Section title="Skills" count={agent?.skills?.length}>
          <SkillList skills={agent?.skills} />
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
