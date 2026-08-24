import { useEffect, useState } from 'react';
import { EMPTY, reduce, type Conversation } from '@chronoai/cma-sdk';
import { AguiConnection, wireTransport, type AguiTransport } from '@chronoai/cma-sdk';
import type { Session } from '../sessions/types';
import './ConversationRecap.css';

/**
 * What a conversation node draws — `Web_Style.md` §13.
 *
 * A recap: the last thing the agent said, what is running, and one key
 * reading `Open to reply`. The previous revision mounted the whole
 * conversation surface inside 420×280 — stream, composer, usage band, all of
 * it — where the composer's placeholder clipped mid-word and the send key had
 * 40px of room. A conversation needs a reading column (§5) and a canvas node
 * cannot give it one, so the node shows what a glance wants and hands the
 * reply to the surface that can hold it.
 *
 * Folded from the same log the surface folds, through the same reducer, so
 * the recap and the surface can never tell two stories about one session.
 */

interface ConversationRecapProps {
  readonly session: Session;
  /** Opens this session on the surface that can hold a reading column. */
  readonly onOpenToReply: () => void;
  /** The wire in production, the mock in development — §1. */
  readonly transport?: AguiTransport;
}

export function ConversationRecap({
  session,
  onOpenToReply,
  transport = wireTransport,
}: ConversationRecapProps) {
  const [conversation, setConversation] = useState<Conversation>(EMPTY);

  useEffect(() => {
    const connection = new AguiConnection(transport, session.id);
    return connection.subscribe(
      (event) => {
        setConversation((state) => reduce(state, event));
      },
      () => {
        // The recap has no link banner: a node is a glance, and the surface
        // it opens onto reports the link when somebody goes to reply.
      },
    );
  }, [session.id, transport]);

  const lastSaid = [...conversation.entries]
    .reverse()
    .find(
      (entry): entry is Extract<(typeof conversation.entries)[number], { kind: 'message' }> =>
        entry.kind === 'message' && entry.role === 'assistant',
    );
  const runningTool = [...conversation.entries]
    .reverse()
    .find(
      (entry): entry is Extract<(typeof conversation.entries)[number], { kind: 'tool' }> =>
        entry.kind === 'tool' && entry.status === undefined,
    );

  return (
    <div className="recap">
      {lastSaid === undefined ? (
        <p className="recap__empty">Nothing said yet.</p>
      ) : (
        <p className="recap__said">{lastSaid.text}</p>
      )}

      {conversation.turnInFlight ? (
        <p className="recap__running">
          <span className="recap__dot pulse" aria-hidden="true" />
          {runningTool === undefined
            ? 'Working'
            : `${runningTool.toolKind}${runningTool.target === undefined ? '' : ` · ${runningTool.target}`}`}
        </p>
      ) : null}

      {/* No composer here, by design: the reply gets a reading column. */}
      <button
        type="button"
        className="button button--ghost button--sm recap__open"
        onClick={onOpenToReply}
      >
        Open to reply
      </button>
    </div>
  );
}
