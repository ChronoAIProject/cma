import { useLayoutEffect, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import type { ConcurrentInput, SessionCapabilities } from '../agui';
import type { AvailableCommand } from './conversation';
import { CommandMenu } from './CommandMenu';
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
  /**
   * Send this turn.
   *
   * Answers whether the turn was accepted, and the composer keeps the draft
   * until it hears yes. It used to clear the field the instant the button was
   * pressed, so a refusal — a `reject` policy mid-turn, a capability the
   * session lacks, a dropped network — took the words with it and left a
   * banner and nothing to retry. On a long run the message that fails is the
   * carefully worded correction.
   *
   * Stated as an answer rather than inferred from a promise settling, because
   * the surface reports refusals rather than throwing them: a promise that
   * resolves is not evidence the service took anything.
   */
  readonly onSend: (text: string) => boolean | Promise<boolean>;
  /**
   * The commands this kernel says it offers, right now — §18.4.
   *
   * Liveness rather than history: the list arrives on the live lane and is
   * absent after a reload until the kernel says again. That is why it belongs
   * here and not in the stream, where §18.1's equivalence rule would have a
   * stale list replayed from the log offering commands that no longer exist.
   */
  readonly availableCommands?: readonly AvailableCommand[];
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
  availableCommands,
  onInterrupt,
  busy = false,
}: ComposerProps) {
  const [text, setText] = useState('');
  const fieldRef = useRef<HTMLTextAreaElement>(null);

  /*
   * The command menu is open when the draft is a lone `/word` — §18.4.
   *
   * Derived from the draft rather than held as its own state, so there is one
   * answer to "is it open" and it cannot disagree with what is in the field.
   * Only at the head of the draft: an agent is asked to do things in prose, and
   * a `/` in the middle of a sentence is a path or a date, not a command.
   */
  const slash = /^\/(\S*)$/.exec(text);
  const [dismissed, setDismissed] = useState(false);
  const commandQuery = slash === null || dismissed ? undefined : (slash[1] ?? '');

  /*
   * Grow the field with what is in it, to §18.4's six lines and no further.
   *
   * §18.4 has required this since it was written, and the comment above the
   * rule in `Composer.css` restates it, but nothing implemented it: the
   * textarea was `rows={1}`, so its intrinsic height sat on the CSS floor and
   * a multi-paragraph prompt scrolled inside a two-line porthole. Somebody
   * writing the careful instruction that a long run turns on could see two
   * lines of it.
   *
   * Measured rather than declared. `field-sizing: content` would do this in
   * CSS, and §9 forbids relying on an ability that has not been demonstrated
   * in the browsers this ships to; a height sync works everywhere. Reset to
   * `auto` first, because `scrollHeight` of an already-tall box never shrinks.
   */
  useLayoutEffect(() => {
    const field = fieldRef.current;
    if (field === null) {
      return;
    }
    field.style.height = 'auto';
    field.style.height = `${String(field.scrollHeight)}px`;
  }, [text]);

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

  /**
   * Hand the turn over, and keep the words until it is taken.
   *
   * Cleared only once `onSend` resolves. A refusal leaves the draft exactly
   * as it was typed, so the reader retries rather than rewrites.
   */
  async function send(): Promise<void> {
    const outgoing = text.trim();
    if (outgoing === '') {
      return;
    }
    try {
      if (await onSend(outgoing)) {
        setText('');
      }
    } catch {
      // Kept, deliberately. The surface reports the refusal; this keeps the
      // only copy of what was written.
    }
  }

  const note = turnInFlight ? POLICY_SAYS[policy] : undefined;

  return (
    <div className="composer-block">
      <div className="composer">
        {commandQuery === undefined ? null : (
          <CommandMenu
            commands={availableCommands}
            query={commandQuery}
            onPick={(command) => {
              // The name and a trailing space, so the caret lands where the
              // argument goes. The reader types what they meant, not what the
              // list happened to spell.
              setText(`/${command.name} `);
              setDismissed(true);
              fieldRef.current?.focus();
            }}
            onDismiss={() => {
              setDismissed(true);
              fieldRef.current?.focus();
            }}
          />
        )}
        <textarea
          className="composer__field"
          aria-label="Message"
          ref={fieldRef}
          rows={1}
          value={text}
          placeholder="Say what you want done"
          onChange={(event) => {
            setText(event.target.value);
            // A draft that stops being a command re-arms the menu, so pressing
            // Escape once does not silence it for the rest of the session.
            if (!event.target.value.startsWith('/')) {
              setDismissed(false);
            }
          }}
          onKeyDown={(event) => {
            // Enter sends; Shift+Enter opens a line.
            // While the menu is open it owns Enter, Escape and the arrows; it
            // listens in the capture phase and stops them reaching here.
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              if (canSend) {
                void send();
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
              void send();
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
