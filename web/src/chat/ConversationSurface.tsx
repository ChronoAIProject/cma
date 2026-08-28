import { useCallback, useEffect, useMemo, useState } from 'react';
import { MessageStream } from './MessageStream';
import { ApprovalCard } from './ApprovalCard';
import { SignInCard } from './SignInCard';
import { Composer } from './Composer';
import { Context } from './Context';
import { Usage } from './Usage';
import { MemoryPressureNotice, Resources } from './Resources';
import { RuntimeLoss } from './RuntimeLoss';
import { initialConversation, reduce, type Conversation } from '@chronoai/cma-sdk';
import { planeStateFor } from './planeState';
import {
  AguiConnection,
  AguiOperations,
  CapabilityRefused,
  fetchTransport,
  wireTransport,
  type AguiTransport,
  type LinkStatus,
  type OperationTransport,
  type SessionCapabilities,
} from '@chronoai/cma-sdk';
import type { Session } from '../sessions/types';
import { readSessionResources, type SessionResourceStatus } from '../api/client';
import './ConversationSurface.css';
import type { ToolCallEntry } from '@chronoai/cma-sdk';

/**
 * A session drawn as a conversation — `Web_Style.md` §18.
 *
 * The counterpart of `TerminalSurface`: the middle column draws one or the
 * other from what the session was granted, and neither knows the other exists.
 *
 * Live and durable state comes from AG-UI. One public REST point read tells an
 * older runtime that will never report from one whose first reading is still
 * due; every third-party client can make the same call (`CLAUDE.md` §1).
 */

interface ConversationSurfaceProps {
  readonly session: Session;
  readonly capabilities: SessionCapabilities;
  /**
   * Where frames come from. The wire in production and the mock in
   * development — §1's "one surface, many drivers", and the reason a component
   * that could tell them apart would be a component that had learned something
   * about its driver it had no business learning.
   */
  readonly transport?: AguiTransport;
  /** Where operations go. Injected for the same reason. */
  readonly operations?: OperationTransport;
  /** The public resource-status point read. Injected so lifecycle tests own it. */
  readonly readResourceStatus?: (
    id: string,
    signal?: AbortSignal,
  ) => Promise<SessionResourceStatus>;
  /**
   * Told when the session starts or stops waiting on an answer — §18.3: a
   * session waiting on one says so everywhere it appears, and the rail and
   * the header are drawn by the console rather than by this surface.
   */
  readonly onWaitingChange?: (waiting: boolean) => void;
  /**
   * Open a tool call in the details column.
   *
   * The surface does not own that column — it belongs to the page — so this
   * reports upward rather than holding a panel of its own.
   */
  readonly onInspect?: (call: ToolCallEntry) => void;
  /** The call the column is showing, when it is showing one. */
  readonly inspectingId?: string;
}

/**
 * How long a link may be down before the surface says so — §18.13.
 *
 * Long enough that an ordinary reconnect passes in silence, short enough that
 * somebody watching a turn that has stopped is not left guessing.
 */
const LINK_LOST_AFTER_MS = 6000;

export function ConversationSurface({
  session,
  capabilities,
  transport = wireTransport,
  operations = fetchTransport,
  readResourceStatus = readSessionResources,
  onWaitingChange,
  onInspect,
  inspectingId,
}: ConversationSurfaceProps) {
  const [conversation, setConversation] = useState<Conversation>(initialConversation);
  const [link, setLink] = useState<LinkStatus>('reconnecting');
  const [busy, setBusy] = useState(false);
  /**
   * Whether the link has been down long enough to be worth saying — §18.13.
   *
   * A transient reconnect says nothing at all: a blip announced as a fault
   * teaches a reader to distrust the next notice, and §11 forbids drawing a
   * recovering session as a failed one. Only a link that stays down past this
   * threshold is named, and then once.
   */
  const [linkLost, setLinkLost] = useState(false);
  const [linkFailure, setLinkFailure] = useState<string | undefined>(undefined);
  const [refusal, setRefusal] = useState<string | undefined>(undefined);
  const [lossDismissed, setLossDismissed] = useState(false);
  const [resourcesUnreported, setResourcesUnreported] = useState(false);

  const client = useMemo(
    () => new AguiOperations(session.id, operations),
    [session.id, operations],
  );

  /*
   * §18.13. The moment the link is not live, stop claiming the agent is
   * working; only after a bounded wait say so in words.
   *
   * The withdrawal is immediate and the notice is not, deliberately: the
   * failure mode this section exists for is the surface *asserting liveness
   * that is false* — the pulse and the caret kept going on a stream that had
   * stopped, so the one affordance built to tell a working agent from a dead
   * one was the one that lied. Withdrawing a false claim is quieter than
   * adding a true one and is correct at every point in between.
   */
  const [lostFor, setLostFor] = useState(link);
  if (lostFor !== link) {
    // Adjusted during render rather than in an effect, which is React's own
    // pattern for a value derived from a changing input: any change of status
    // restarts the wait, so a link that drops, recovers and drops again gets
    // the full threshold each time rather than being named instantly.
    setLostFor(link);
    setLinkLost(false);
  }

  useEffect(() => {
    if (link !== 'reconnecting') {
      return;
    }
    const timer = setTimeout(() => {
      setLinkLost(true);
    }, LINK_LOST_AFTER_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [link]);

  useEffect(() => {
    // One connection per session, opened on mount and closed on unmount. The
    // conversation is not cleared here: the surface is keyed on the session at
    // its call sites, so a different session is a different component rather
    // than this one being emptied and refilled.
    const controller = new AbortController();
    let closed = false;
    let hasBeenLive = false;
    let latestStatusRead = 0;

    const refreshResourceStatus = async () => {
      const read = ++latestStatusRead;
      try {
        const status = await readResourceStatus(session.id, controller.signal);
        if (!closed && read === latestStatusRead) {
          setResourcesUnreported(status.reason === 'runtime_does_not_report');
        }
      } catch {
        // The stream remains authoritative for readings. A failed point read
        // cannot turn an absence into either of the two sentences it distinguishes.
      }
    };

    if (capabilities.resources) {
      void refreshResourceStatus();
    }

    const connection = new AguiConnection(transport, session.id);
    const unsubscribe = connection.subscribe(
      (event) => {
        setConversation((state) => reduce(state, event));
      },
      (status, failure) => {
        setLink(status);
        if (status === 'failed') {
          const detail = failure?.message ?? 'the control plane gave no reason';
          setLinkFailure(
            failure?.status === undefined
              ? `The session event stream failed: ${detail}`
              : `The session event stream was refused (HTTP ${String(failure.status)}): ${detail}`,
          );
        }
        if (status === 'live') {
          setLinkFailure(undefined);
          // The first live edge belongs to the mount read above. Every later
          // one follows a real drop and gets one fresh answer, never a poll.
          if (hasBeenLive && capabilities.resources) {
            void refreshResourceStatus();
          }
          hasBeenLive = true;
        }
      },
    );
    return () => {
      closed = true;
      controller.abort();
      unsubscribe();
    };
  }, [capabilities.resources, readResourceStatus, session.id, transport]);

  /**
   * Run an operation and keep its refusal intact.
   *
   * `API_Overview.md` rule 4 — errors name what is missing — is worth nothing
   * if the interface renders it as "something went wrong", so the capability
   * name reaches the surface (§18.7).
   */
  /**
   * Run one thing the reader asked for, and say whether it was taken.
   *
   * The answer matters to the composer, which keeps the draft until the
   * service has accepted the turn: a refusal here is reported in the banner
   * rather than thrown, so settling is not evidence of success.
   */
  const attempt = useCallback(async (work: () => Promise<void>): Promise<boolean> => {
    setBusy(true);
    setRefusal(undefined);
    try {
      await work();
      return true;
    } catch (cause) {
      setRefusal(
        cause instanceof CapabilityRefused
          ? `This session cannot ${cause.capability}: ${cause.message}`
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
      return false;
    } finally {
      setBusy(false);
    }
  }, []);

  const pending = conversation.pending;
  /*
   * §11: `idle` is read-only on this plane *unless* it is waiting on an
   * approval, which is the most alive a session gets. The composer stays
   * refused either way — an approval is answered by its own card — but the
   * refusal says the agent stopped here on purpose, and the card is drawn.
   */
  const state = planeStateFor(session, pending !== undefined);

  /*
   * Lifted so the rail line and the header pill can say the session is
   * waiting for its owner (§18.3) — only this surface folds the log, so only
   * it knows an approval is parked.
   */
  useEffect(() => {
    onWaitingChange?.(state.waiting);
  }, [state.waiting, onWaitingChange]);

  const reason =
    state.reason ??
    (link === 'reconnecting' ? 'Reconnecting to this session.' : undefined) ??
    linkFailure ??
    refusal;

  const visibleFailure = linkFailure ?? refusal;

  return (
    <section className="conversation" aria-label="Conversation">
      <RuntimeLoss
        loss={lossDismissed ? undefined : conversation.runtimeLoss}
        onDismiss={() => {
          setLossDismissed(true);
        }}
      />

      <MemoryPressureNotice pressure={conversation.memoryPressure} />

      {visibleFailure === undefined ? null : (
        <div className="notice notice--fail conversation__refusal" role="alert">
          <span className="notice__detail">{visibleFailure}</span>
        </div>
      )}

      <MessageStream
        entries={conversation.entries}
        onInspect={onInspect}
        inspectingId={inspectingId}
        turnInFlight={conversation.turnInFlight}
        live={conversation.live}
        reasoning={conversation.reasoning}
        plan={conversation.plan}
        mode={conversation.mode}
        configOptions={conversation.configOptions}
        linkLive={link === 'live'}
        linkLost={linkLost}
        footer={
          /*
           * The sign-in stays in the stream: it is a thing to go and do
           * somewhere else, and a person can leave it open while they read.
           * The approval does not — see the composer seat below.
           */
          conversation.signIn === undefined ? null : <SignInCard signIn={conversation.signIn} />
        }
      />

      {/*
       * The composer card at the foot of the reading column, and usage
       * beneath it, outside its card — §18.4 and §18.5. The same centred
       * column as the stream, so the card sits under the turns it answers.
       */}
      <div className="conversation__foot">
        <div className="conversation__footcolumn">
          {/*
           * An approval takes the composer's seat rather than sitting in the
           * stream — §18.3, amended.
           *
           * The section's binding constraint was that it must never be a
           * modal, so that a session waiting on an answer leaves every other
           * session reachable. That still holds: this is inside one session's
           * own surface and the sidebar is untouched.
           *
           * What changes is that an approval in the stream scrolls away. A
           * turn that stopped five entries ago leaves its question five
           * entries up, and the one thing a person must answer is the one
           * thing they can scroll past. Here it is where they were already
           * looking, and it replaces the field because typing is not what the
           * session is waiting for.
           */}
          {pending === undefined ? (
            <Composer
              capabilities={capabilities}
              availableCommands={conversation.availableCommands}
              turnInFlight={conversation.turnInFlight}
              refusedReason={state.interactive ? undefined : reason}
              busy={busy}
              onSend={(text) => attempt(() => client.prompt(text, runId()))}
              onInterrupt={() => {
                void attempt(() => client.interrupt());
              }}
            />
          ) : (
            <ApprovalCard
              interrupt={pending}
              busy={busy}
              /*
               * The outcome goes back to the card, which is showing the answer
               * already. A refusal has to put the question back, or the reader
               * is left looking at a decision the service never took.
               */
              onAnswer={(decision) =>
                attempt(() => client.resolveApproval(pending.id, decision, runId()))
              }
            />
          )}
          <Usage granted={capabilities.usage} usage={conversation.usage} />
          <Resources
            granted={capabilities.resources}
            unreported={resourcesUnreported}
            resources={conversation.resources}
            pressure={conversation.memoryPressure}
          />
          <Context granted={capabilities.usage} usage={conversation.usage} />
        </div>
      </div>
    </section>
  );
}

/**
 * A client-generated identifier for one turn.
 *
 * `docs/API_AGUI.md` §2: the client chooses it and every event on the run
 * echoes it back. Random rather than counted, because two tabs driving one
 * session must not produce the same one.
 */
function runId(): string {
  return `run_${Math.random().toString(36).slice(2, 10)}`;
}
