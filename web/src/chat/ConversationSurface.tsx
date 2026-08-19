import { useCallback, useEffect, useMemo, useState } from 'react';
import { MessageStream } from './MessageStream';
import { ApprovalCard } from './ApprovalCard';
import { Composer } from './Composer';
import { Usage } from './Usage';
import { EMPTY, reduce, type Conversation } from './conversation';
import { planeStateFor } from './planeState';
import {
  AguiConnection,
  AguiOperations,
  AguiRefusal,
  fetchTransport,
  wireTransport,
  type AguiTransport,
  type LinkStatus,
  type OperationTransport,
  type SessionCapabilities,
} from '../agui';
import type { Session } from '../sessions/types';
import './ConversationSurface.css';

/**
 * A session drawn as a conversation — `Web_Style.md` §18.
 *
 * The counterpart of `TerminalSurface`: the middle column draws one or the
 * other from what the session was granted, and neither knows the other exists.
 *
 * Everything it shows comes from AG-UI and from nothing else. A chat surface
 * reaching for a terminal route to fill a gap in the conversation has found a
 * hole in AG-UI, and the hole is the bug (`CLAUDE.md` §1).
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
  /**
   * Told when the session starts or stops waiting on an answer — §18.3: a
   * session waiting on one says so everywhere it appears, and the rail and
   * the header are drawn by the console rather than by this surface.
   */
  readonly onWaitingChange?: (waiting: boolean) => void;
}

export function ConversationSurface({
  session,
  capabilities,
  transport = wireTransport,
  operations = fetchTransport,
  onWaitingChange,
}: ConversationSurfaceProps) {
  const [conversation, setConversation] = useState<Conversation>(EMPTY);
  const [link, setLink] = useState<LinkStatus>('reconnecting');
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | undefined>(undefined);

  const client = useMemo(
    () => new AguiOperations(session.id, operations),
    [session.id, operations],
  );

  useEffect(() => {
    // One connection per session, opened on mount and closed on unmount. The
    // conversation is not cleared here: the surface is keyed on the session at
    // its call sites, so a different session is a different component rather
    // than this one being emptied and refilled.
    const connection = new AguiConnection(transport, session.id);
    return connection.subscribe(
      (event) => {
        setConversation((state) => reduce(state, event));
      },
      (status) => {
        setLink(status);
      },
    );
  }, [session.id, transport]);

  /**
   * Run an operation and keep its refusal intact.
   *
   * `API_Overview.md` rule 4 — errors name what is missing — is worth nothing
   * if the interface renders it as "something went wrong", so the capability
   * name reaches the surface (§18.7).
   */
  const attempt = useCallback(async (work: () => Promise<void>) => {
    setBusy(true);
    setRefusal(undefined);
    try {
      await work();
    } catch (cause) {
      setRefusal(
        cause instanceof AguiRefusal && cause.capability !== undefined
          ? `This session cannot ${cause.capability}: ${cause.message}`
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
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
    refusal;

  return (
    <section className="conversation" aria-label="Conversation">
      {refusal === undefined ? null : (
        <div className="notice notice--fail conversation__refusal" role="alert">
          <span className="notice__detail">{refusal}</span>
        </div>
      )}

      <MessageStream
        entries={conversation.entries}
        kernel={session.kernel}
        turnInFlight={conversation.turnInFlight}
        live={conversation.live}
        reasoning={conversation.reasoning}
        plan={conversation.plan}
        mode={conversation.mode}
        configOptions={conversation.configOptions}
        availableCommands={conversation.availableCommands}
        footer={
          pending === undefined ? null : (
            <ApprovalCard
              interrupt={pending}
              busy={busy}
              onAnswer={(decision) => {
                void attempt(() => client.resolveApproval(pending.id, decision, runId()));
              }}
            />
          )
        }
      />

      {/*
       * The composer card at the foot of the reading column, and usage
       * beneath it, outside its card — §18.4 and §18.5. The same centred
       * column as the stream, so the card sits under the turns it answers.
       */}
      <div className="conversation__foot">
        <div className="conversation__footcolumn">
          <Composer
            capabilities={capabilities}
            turnInFlight={conversation.turnInFlight}
            refusedReason={state.interactive ? undefined : reason}
            busy={busy}
            onSend={(text) => {
              void attempt(() => client.prompt(text, runId()));
            }}
            onInterrupt={() => {
              void attempt(() => client.interrupt());
            }}
          />
          <Usage granted={capabilities.usage} usage={conversation.usage} />
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
