import { useState } from 'react';
import { Button } from '../ui/Button';
import type { ConcurrentInput, SessionCapabilities } from '../agui';
import './Composer.css';

/**
 * Where a prompt is written and sent — `Web_Style.md` §18.4.
 *
 * What it does while a turn is in flight is **not a style choice**.
 * `API_Overview.md` §6 rule 5: a second turn arriving mid-turn resolves
 * identically on every interface, under a policy chosen per session at
 * creation and reported in its capability document. The composer reads that
 * policy and behaves accordingly; it does not invent one, and it does not
 * disable itself on a guess.
 *
 * Steering is separate and is a granted capability. A session not granted it
 * shows **no steering affordance at all** rather than a disabled one, because
 * there is nothing about the session's state to explain and nothing that would
 * ever enable it: a control that can never work is not a control that is
 * currently unavailable.
 *
 * Interrupt lives here because it is what somebody reaches for in the same
 * moment and for the same reason.
 */

interface ComposerProps {
  readonly capabilities: SessionCapabilities;
  readonly turnInFlight: boolean;
  /** Why the session cannot take input at all, when it cannot (§11). */
  readonly refusedReason?: string;
  readonly onSend: (text: string) => void;
  readonly onInterrupt: () => void;
  readonly busy?: boolean;
}

/** What each policy means for the person typing, in their terms. */
const POLICY_SAYS: Record<ConcurrentInput, string> = {
  reject: 'A turn is in progress. Wait for it to finish.',
  enqueue: 'This will be sent after the turn in progress.',
  interrupt: 'Sending this will stop the turn in progress.',
  rollback: 'Sending this will discard the turn in progress.',
};

export function Composer({
  capabilities,
  turnInFlight,
  refusedReason,
  onSend,
  onInterrupt,
  busy = false,
}: ComposerProps) {
  const [text, setText] = useState('');
  const policy = capabilities.concurrentInput;

  /*
   * §11: a session that cannot be typed into refuses, and says why. It is
   * never merely inert — a composer that has quietly stopped working is the
   * thing people report as broken.
   */
  const refused = refusedReason !== undefined;
  /*
   * The only policy that stops a turn being sent. The other three all send;
   * what differs is what happens to the one already running, which is the
   * sentence below rather than a disabled key.
   */
  const heldByTurn = turnInFlight && policy === 'reject';
  const canSend = !refused && !heldByTurn && !busy && text.trim().length > 0;

  /*
   * §18.4: a composer on a session that cannot take input refuses, and says
   * why — a dashed-border card carrying the reason and what to do instead.
   * It is never merely inert.
   */
  if (refused) {
    return (
      <div className="composer composer--refused">
        <p className="composer__refusal" role="status">
          {refusedReason}
        </p>
      </div>
    );
  }

  const note = turnInFlight ? POLICY_SAYS[policy] : undefined;

  return (
    <div className="composer-block">
      <div className="composer">
        <textarea
          className="composer__field"
          aria-label="Message"
          rows={1}
          value={text}
          placeholder="Say what you want done"
          onChange={(event) => {
            setText(event.target.value);
          }}
          onKeyDown={(event) => {
            // Enter sends; Shift+Enter opens a line.
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              if (canSend) {
                onSend(text.trim());
                setText('');
              }
            }
          }}
        />

        {/*
         * The card's control row: what the session's own policy means, then
         * interrupt while a turn is in flight, then the primary send. The
         * sentence is on the surface beside the key, never in a tooltip (§7).
         */}
        <div className="composer__row">
          {note === undefined ? null : (
            <span className="composer__note" role="status">
              {note}
            </span>
          )}
          {/*
           * §18.4: a session without `steer` shows no steering affordance at
           * all. Where it was granted, it is stated as a tag on the
           * composer's own row.
           */}
          {capabilities.steer && turnInFlight ? (
            <span className="tag composer__steer">steerable</span>
          ) : null}
          <span className="composer__spacer" />

          {/*
           * Interrupt is offered only while a turn is in flight, and only on
           * a session granted it — a kernel that cannot withdraw a prompt is
           * refused by name rather than left waiting for something that will
           * not happen.
           */}
          {turnInFlight && capabilities.interrupt ? (
            <Button disabled={busy} onClick={onInterrupt}>
              Interrupt
            </Button>
          ) : null}

          <Button
            tone="accent"
            disabled={!canSend}
            onClick={() => {
              onSend(text.trim());
              setText('');
            }}
          >
            Send
            <kbd className="composer__hint" aria-hidden="true">
              ↵
            </kbd>
          </Button>
        </div>
      </div>

      {/* Stated once, beneath the card — §18.4. */}
      <p className="composer__keys">Enter sends; Shift+Enter opens a line.</p>
    </div>
  );
}
